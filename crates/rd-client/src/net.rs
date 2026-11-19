//! Background connection to the rendezvous server.
//!
//! Runs on its own thread with a small Tokio runtime, reconnects with backoff,
//! and hands server messages to the UI through a channel.
//!
//! Peer messages for sessions with a registered route (the screen-share
//! thread's frame acknowledgements) bypass the UI, so sharing keeps flowing
//! even while the window is minimized and not repainting.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc as std_mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use rd_core::{
    media::{PeerMessage, decode_screen_frame},
    protocol::{ClientMessage, ErrorCode, PROTOCOL_VERSION, ServerMessage},
};
use tokio::{
    sync::mpsc,
    time::{MissedTickBehavior, interval, sleep, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(25);
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub enum NetEvent {
    Connecting,
    Message(ServerMessage),
    /// A peer message for a session without a route (handled by the UI).
    Peer { session_id: Uuid, message: PeerMessage },
    ScreenFrame { session_id: Uuid, seq: u32, jpeg: Vec<u8> },
    /// The connection dropped; it will be retried unless `retrying` is false.
    Disconnected { reason: String, retrying: bool },
}

enum Command {
    Json(ClientMessage),
    Binary(Vec<u8>),
}

type Routes = Arc<Mutex<HashMap<Uuid, std_mpsc::Sender<PeerMessage>>>>;

/// Cloneable handle for sending to the server from any thread.
#[derive(Clone)]
pub struct NetSender {
    commands: mpsc::UnboundedSender<Command>,
    routes: Routes,
}

impl NetSender {
    pub fn send(&self, message: ClientMessage) {
        let _ = self.commands.send(Command::Json(message));
    }

    pub fn send_binary(&self, frame: Vec<u8>) {
        let _ = self.commands.send(Command::Binary(frame));
    }

    pub fn send_peer(&self, session_id: Uuid, message: &PeerMessage) {
        let payload = serde_json::to_value(message).expect("peer messages always serialize");
        self.send(ClientMessage::Relay { session_id, payload });
    }

    /// Delivers this session's peer messages to `tx` instead of the UI.
    pub fn add_route(&self, session_id: Uuid, tx: std_mpsc::Sender<PeerMessage>) {
        lock(&self.routes).insert(session_id, tx);
    }

    pub fn remove_route(&self, session_id: Uuid) {
        lock(&self.routes).remove(&session_id);
    }
}

fn lock(routes: &Routes) -> std::sync::MutexGuard<'_, HashMap<Uuid, std_mpsc::Sender<PeerMessage>>> {
    routes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct NetHandle {
    pub sender: NetSender,
    pub events: std_mpsc::Receiver<NetEvent>,
}

impl NetHandle {
    pub fn send(&self, message: ClientMessage) {
        self.sender.send(message);
    }
}

pub fn spawn(server_url: String, device_secret: String, ctx: egui::Context) -> NetHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = std_mpsc::channel();
    let routes: Routes = Arc::default();
    let emitter = Emitter { tx: event_tx, ctx };

    let thread_routes = routes.clone();
    thread::Builder::new()
        .name("network".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to start network runtime");
            runtime.block_on(run(server_url, device_secret, command_rx, emitter, thread_routes));
        })
        .expect("failed to spawn network thread");

    NetHandle {
        sender: NetSender { commands: command_tx, routes },
        events: event_rx,
    }
}

struct Emitter {
    tx: std_mpsc::Sender<NetEvent>,
    ctx: egui::Context,
}

impl Emitter {
    /// Returns false once the UI has gone away.
    fn emit(&self, event: NetEvent) -> bool {
        let delivered = self.tx.send(event).is_ok();
        self.ctx.request_repaint();
        delivered
    }
}

enum Stop {
    /// The UI closed; exit quietly.
    AppClosed,
    /// Try again after a delay.
    Retry(String),
    /// Retrying would not help (e.g. another copy of the app took over this ID).
    Fatal(String),
}

async fn run(
    url: String,
    secret: String,
    mut commands: mpsc::UnboundedReceiver<Command>,
    emitter: Emitter,
    routes: Routes,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if !emitter.emit(NetEvent::Connecting) {
            return;
        }
        let stop = connection(&url, &secret, &mut commands, &emitter, &routes, &mut backoff).await;
        // Every session ends with the connection; stop any screen sharing.
        lock(&routes).clear();
        match stop {
            Stop::AppClosed => return,
            Stop::Fatal(reason) => {
                emitter.emit(NetEvent::Disconnected { reason, retrying: false });
                // Stay idle until the app exits.
                while commands.recv().await.is_some() {}
                return;
            }
            Stop::Retry(reason) => {
                if !emitter.emit(NetEvent::Disconnected { reason, retrying: true }) {
                    return;
                }
            }
        }
        sleep(backoff).await;
        // Anything queued while offline refers to sessions that no longer exist.
        while commands.try_recv().is_ok() {}
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn to_frame(message: &ClientMessage) -> Message {
    Message::Text(
        serde_json::to_string(message)
            .expect("client messages always serialize")
            .into(),
    )
}

async fn connection(
    url: &str,
    secret: &str,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    emitter: &Emitter,
    routes: &Routes,
    backoff: &mut Duration,
) -> Stop {
    let socket = match timeout(CONNECT_TIMEOUT, connect_async(url)).await {
        Err(_) => return Stop::Retry("Timed out reaching the server.".into()),
        Ok(Err(error)) => return Stop::Retry(format!("Could not reach the server: {error}")),
        Ok(Ok((socket, _))) => socket,
    };
    let (mut sink, mut stream) = socket.split();

    let register = ClientMessage::Register {
        protocol_version: PROTOCOL_VERSION,
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        device_secret: secret.to_owned(),
    };
    if let Err(error) = sink.send(to_frame(&register)).await {
        return Stop::Retry(format!("Could not register: {error}"));
    }

    let mut ping = interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let frame = match command {
                    None => return Stop::AppClosed,
                    Some(Command::Json(message)) => to_frame(&message),
                    Some(Command::Binary(data)) => Message::Binary(data.into()),
                };
                if let Err(error) = sink.send(frame).await {
                    return Stop::Retry(format!("Lost connection: {error}"));
                }
            }
            _ = ping.tick() => {
                if let Err(error) = sink.send(Message::Ping(Default::default())).await {
                    return Stop::Retry(format!("Lost connection: {error}"));
                }
            }
            frame = timeout(IDLE_TIMEOUT, stream.next()) => {
                let frame = match frame {
                    Err(_) => return Stop::Retry("The server stopped responding.".into()),
                    Ok(None) | Ok(Some(Ok(Message::Close(_)))) => {
                        return Stop::Retry("The server closed the connection.".into());
                    }
                    Ok(Some(Err(error))) => return Stop::Retry(format!("Lost connection: {error}")),
                    Ok(Some(Ok(frame))) => frame,
                };
                let event = match frame {
                    Message::Text(text) => {
                        let Ok(message) = serde_json::from_str::<ServerMessage>(text.as_str()) else {
                            continue;
                        };
                        match handle_server_message(message, routes, backoff) {
                            Ok(Some(event)) => event,
                            Ok(None) => continue,
                            Err(stop) => return stop,
                        }
                    }
                    Message::Binary(data) => match decode_screen_frame(&data) {
                        Some(frame) => NetEvent::ScreenFrame {
                            session_id: frame.session_id,
                            seq: frame.seq,
                            jpeg: frame.jpeg.to_vec(),
                        },
                        None => continue,
                    },
                    _ => continue,
                };
                if !emitter.emit(event) {
                    return Stop::AppClosed;
                }
            }
        }
    }
}

/// Decides what a server message means for the UI and for routed sessions.
fn handle_server_message(
    message: ServerMessage,
    routes: &Routes,
    backoff: &mut Duration,
) -> Result<Option<NetEvent>, Stop> {
    match &message {
        ServerMessage::Registered { .. } => *backoff = Duration::from_secs(1),
        ServerMessage::Error {
            code: ErrorCode::Replaced | ErrorCode::UnsupportedVersion | ErrorCode::InvalidSecret,
            message: text,
            ..
        } => return Err(Stop::Fatal(text.clone())),
        ServerMessage::SessionEnded { session_id, .. } => {
            // Dropping the route stops screen sharing for this session.
            lock(routes).remove(session_id);
        }
        ServerMessage::Relay { session_id, payload } => {
            let Ok(peer) = serde_json::from_value::<PeerMessage>(payload.clone()) else {
                return Ok(None);
            };
            let session_id = *session_id;
            if let Some(route) = lock(routes).get(&session_id) {
                let _ = route.send(peer);
                return Ok(None);
            }
            return Ok(Some(NetEvent::Peer { session_id, message: peer }));
        }
        _ => {}
    }
    Ok(Some(NetEvent::Message(message)))
}
