//! Messages exchanged between the two apps inside a session.
//!
//! The server never looks inside these: JSON `PeerMessage`s travel as the
//! `payload` of `relay` messages, and screen frames travel as binary frames.
//!
//! Binary screen frame layout:
//!
//! ```text
//! [16 bytes session ID][1 byte kind = 1][4 bytes sequence, big-endian][JPEG data]
//! ```

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::BINARY_HEADER_LEN;

pub const KIND_SCREEN_JPEG: u8 = 1;
pub const SCREEN_HEADER_LEN: usize = BINARY_HEADER_LEN + 1 + 4;

/// Control messages between the two apps, sent as relay payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerMessage {
    /// The viewer has displayed frame `seq`; the host may send more.
    FrameAck { seq: u32 },
    /// The host could not share its screen.
    ShareError { message: String },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ScreenFrame<'a> {
    pub session_id: Uuid,
    pub seq: u32,
    pub jpeg: &'a [u8],
}

pub fn encode_screen_frame(session_id: Uuid, seq: u32, jpeg: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(SCREEN_HEADER_LEN + jpeg.len());
    frame.extend_from_slice(session_id.as_bytes());
    frame.push(KIND_SCREEN_JPEG);
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(jpeg);
    frame
}

/// Parses a binary frame, or `None` if it is not a screen frame.
pub fn decode_screen_frame(data: &[u8]) -> Option<ScreenFrame<'_>> {
    if data.len() < SCREEN_HEADER_LEN || data[BINARY_HEADER_LEN] != KIND_SCREEN_JPEG {
        return None;
    }
    let session_id = Uuid::from_slice(&data[..BINARY_HEADER_LEN]).ok()?;
    let seq_bytes: [u8; 4] = data[BINARY_HEADER_LEN + 1..SCREEN_HEADER_LEN].try_into().ok()?;
    Some(ScreenFrame {
        session_id,
        seq: u32::from_be_bytes(seq_bytes),
        jpeg: &data[SCREEN_HEADER_LEN..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_frame_round_trips() {
        let id = Uuid::new_v4();
        let encoded = encode_screen_frame(id, 42, b"jpeg");
        assert_eq!(
            decode_screen_frame(&encoded),
            Some(ScreenFrame { session_id: id, seq: 42, jpeg: b"jpeg" })
        );
    }

    #[test]
    fn other_binary_frames_are_ignored() {
        assert_eq!(decode_screen_frame(&[0; 10]), None);
        let mut encoded = encode_screen_frame(Uuid::nil(), 1, b"x");
        encoded[BINARY_HEADER_LEN] = 99;
        assert_eq!(decode_screen_frame(&encoded), None);
    }

    #[test]
    fn peer_messages_are_tagged() {
        let json = serde_json::to_string(&PeerMessage::FrameAck { seq: 7 }).unwrap();
        assert_eq!(json, r#"{"kind":"frame_ack","seq":7}"#);
    }
}
