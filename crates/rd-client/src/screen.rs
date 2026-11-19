//! Screen sharing.
//!
//! Host side: a background thread captures the primary monitor, encodes it as
//! JPEG and sends it to the viewer. It sends a new frame only when the screen
//! changed and the viewer has acknowledged enough earlier frames, so a slow
//! link gets fewer frames instead of an ever-growing backlog.
//!
//! Viewer side: [`decode_jpeg`] turns a received frame into an egui image.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use rd_core::media::{PeerMessage, encode_screen_frame};
use uuid::Uuid;

use crate::net::NetSender;

/// Target frame interval (about 10 frames per second at most).
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
/// Frames that may be on their way to the viewer at once.
const MAX_IN_FLIGHT: u32 = 2;
/// If the viewer acknowledges nothing for this long, assume frames were lost.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);
/// Wider screens are halved until they fit, to keep frames small.
const MAX_WIDTH: u32 = 1920;
const JPEG_QUALITY: u8 = 60;

/// Stops sharing when dropped.
pub struct ShareHandle {
    stop: Arc<AtomicBool>,
}

impl Drop for ShareHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Starts sending this PC's screen to the viewer of `session_id`.
pub fn start_sharing(session_id: Uuid, net: NetSender) -> ShareHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (peer_tx, peer_rx) = mpsc::channel();
    net.add_route(session_id, peer_tx);

    let thread_stop = stop.clone();
    let spawned = thread::Builder::new()
        .name("screen-share".into())
        .spawn(move || {
            if let Err(error) = share_loop(session_id, &net, &peer_rx, &thread_stop) {
                net.send_peer(session_id, &PeerMessage::ShareError { message: error });
            }
            net.remove_route(session_id);
        });
    if spawned.is_err() {
        stop.store(true, Ordering::Relaxed);
    }
    ShareHandle { stop }
}

fn share_loop(
    session_id: Uuid,
    net: &NetSender,
    peer_rx: &mpsc::Receiver<PeerMessage>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let monitor = primary_monitor()?;
    let mut sent: u32 = 0;
    let mut acked: u32 = 0;
    let mut previous: Vec<u8> = Vec::new();
    let mut next_capture = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        // Collect acknowledgements without waiting.
        loop {
            match peer_rx.try_recv() {
                Ok(PeerMessage::FrameAck { seq }) => acked = acked.max(seq),
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                // The session ended or the connection dropped.
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Too many frames unacknowledged: wait for the viewer to catch up.
        if sent.wrapping_sub(acked) >= MAX_IN_FLIGHT {
            match peer_rx.recv_timeout(ACK_TIMEOUT) {
                Ok(PeerMessage::FrameAck { seq }) => acked = acked.max(seq),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    // Assume the frames were lost; resend the current screen.
                    acked = sent;
                    previous.clear();
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
            continue;
        }

        let now = Instant::now();
        if now < next_capture {
            thread::sleep(next_capture - now);
        }
        next_capture = Instant::now() + FRAME_INTERVAL;

        let image = monitor
            .capture_image()
            .map_err(|error| format!("Could not capture the screen: {error}"))?;
        let (width, height) = image.dimensions();
        let pixels = image.into_raw();
        if pixels == previous {
            continue; // Nothing changed on screen.
        }

        let (scaled, scaled_width, scaled_height) = downscale(&pixels, width, height);
        let jpeg = encode_jpeg(&scaled, scaled_width, scaled_height)?;
        sent = sent.wrapping_add(1);
        net.send_binary(encode_screen_frame(session_id, sent, &jpeg));
        previous = pixels;
    }
    Ok(())
}

fn primary_monitor() -> Result<xcap::Monitor, String> {
    let monitors =
        xcap::Monitor::all().map_err(|error| format!("Could not list monitors: {error}"))?;
    let primary = monitors
        .iter()
        .position(|monitor| monitor.is_primary().unwrap_or(false))
        .unwrap_or(0);
    monitors
        .into_iter()
        .nth(primary)
        .ok_or_else(|| "No monitor found to share.".to_owned())
}

/// Halves an RGBA image (2×2 box filter) until it is at most `MAX_WIDTH` wide.
fn downscale(pixels: &[u8], width: u32, height: u32) -> (std::borrow::Cow<'_, [u8]>, u32, u32) {
    let mut current = std::borrow::Cow::Borrowed(pixels);
    let (mut w, mut h) = (width, height);
    while w > MAX_WIDTH && h >= 2 {
        let (nw, nh) = (w / 2, h / 2);
        let mut out = vec![0u8; (nw * nh * 4) as usize];
        let src = &current;
        let stride = (w * 4) as usize;
        for y in 0..nh as usize {
            let row0 = &src[(2 * y) * stride..];
            let row1 = &src[(2 * y + 1) * stride..];
            for x in 0..nw as usize {
                let i = x * 8;
                let o = (y * nw as usize + x) * 4;
                for c in 0..4 {
                    let sum = row0[i + c] as u16
                        + row0[i + 4 + c] as u16
                        + row1[i + c] as u16
                        + row1[i + 4 + c] as u16;
                    out[o + c] = (sum / 4) as u8;
                }
            }
        }
        current = std::borrow::Cow::Owned(out);
        (w, h) = (nw, nh);
    }
    (current, w, h)
}

fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let too_big = || "Screen is too large to encode.".to_owned();
    let mut jpeg = Vec::with_capacity(256 * 1024);
    let encoder = jpeg_encoder::Encoder::new(&mut jpeg, JPEG_QUALITY);
    encoder
        .encode(
            rgba,
            u16::try_from(width).map_err(|_| too_big())?,
            u16::try_from(height).map_err(|_| too_big())?,
            jpeg_encoder::ColorType::Rgba,
        )
        .map_err(|error| format!("Could not encode the screen: {error}"))?;
    Ok(jpeg)
}

/// Decodes a received JPEG frame into an image egui can display.
pub fn decode_jpeg(jpeg: &[u8]) -> Result<egui::ColorImage, String> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder.decode().map_err(|error| format!("Bad frame: {error:?}"))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| "Bad frame: no dimensions".to_owned())?;
    if pixels.len() != width * height * 4 {
        return Err("Bad frame: unexpected size".to_owned());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_round_trip() {
        let (w, h) = (64u32, 48u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % 256) as u8, 128, 64, 255]).collect();
        let jpeg = encode_jpeg(&rgba, w, h).unwrap();
        let image = decode_jpeg(&jpeg).unwrap();
        assert_eq!(image.size, [w as usize, h as usize]);
    }

    #[test]
    fn downscale_halves_wide_screens() {
        let (w, h) = (3840u32, 2160u32);
        let rgba = vec![200u8; (w * h * 4) as usize];
        let (out, nw, nh) = downscale(&rgba, w, h);
        assert_eq!((nw, nh), (1920, 1080));
        assert_eq!(out.len(), (nw * nh * 4) as usize);
        assert!(out.iter().all(|&v| v == 200));

        let small = vec![1u8; 100 * 50 * 4];
        let (out, nw, nh) = downscale(&small, 100, 50);
        assert_eq!((nw, nh, out.len()), (100, 50, small.len()));
    }
}
