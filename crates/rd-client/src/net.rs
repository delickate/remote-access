//! Background connection to the rendezvous server.
//!
//! Runs on its own thread with a small Tokio runtime, reconnects with backoff,
//! and hands server messages to the UI through a channel.

use std::{sync::mpsc as std_mpsc, thread, time::Duration};

use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use rd_core::protocol::{ClientMessage, ErrorCode, PROTOCOL_VERSION, ServerMessage};
use tokio::{
    sync::mpsc,
    time::{MissedTickBehavior, interval, sleep, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(25);
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub enum NetEvent {
    Connecting,
    Message(ServerMessage),
    /// The connection dropped; it will be retried unless `retrying` is false.
    Disconnected { reason: String, retrying: bool },
}

pub struct NetHandle {
    commands: mpsc::UnboundedSender<ClientMessage>,
    pub events: std_mpsc::Receiver<NetEvent>,
}

impl NetHandle {
    pub fn send(&self, message: ClientMessage) {
        let _ = self.commands.send(message);
    }
}

pub fn spawn(server_url: String, device_secret: String, ctx: egui::Context) -> NetHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = std_mpsc::channel();
    let emitter = Emitter { tx: event_tx, ctx };

    thread::Builder::new()
        .name("network".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to start network runtime");
            runtime.block_on(run(server_url, device_secret, command_rx, emitter));
        })
        .expect("failed to spawn network thread");

    NetHandle { commands: command_tx, events: event_rx }
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
    mut commands: mpsc::UnboundedReceiver<ClientMessage>,
    emitter: Emitter,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if !emitter.emit(NetEvent::Connecting) {
            return;
        }
        match connection(&url, &secret, &mut commands, &emitter, &mut backoff).await {
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
    commands: &mut mpsc::UnboundedReceiver<ClientMessage>,
    emitter: &Emitter,
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
                let Some(command) = command else { return Stop::AppClosed };
                if let Err(error) = sink.send(to_frame(&command)).await {
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
                let Message::Text(text) = frame else { continue };
                let Ok(message) = serde_json::from_str::<ServerMessage>(text.as_str()) else {
                    continue;
                };

                match &message {
                    ServerMessage::Registered { .. } => *backoff = Duration::from_secs(1),
                    ServerMessage::Error {
                        code: ErrorCode::Replaced | ErrorCode::UnsupportedVersion | ErrorCode::InvalidSecret,
                        message: text,
                        ..
                    } => {
                        return Stop::Fatal(text.clone());
                    }
                    _ => {}
                }
                if !emitter.emit(NetEvent::Message(message)) {
                    return Stop::AppClosed;
                }
            }
        }
    }
}

