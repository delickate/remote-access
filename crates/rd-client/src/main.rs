// Hide the extra console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke,
    TextStyle, Vec2,
};
use rd_core::{
    identity::device_id_from_secret,
    protocol::{ClientMessage, EndReason, Role, ServerMessage},
};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

mod net;

const APP_TITLE: &str = "Delickate Remote Access";
const COPY_FEEDBACK_SECS: f64 = 1.5;

/// Server used when nothing overrides it. Bake in your own at build time with
/// `RD_SERVER_URL=wss://your-domain/ws cargo build --release`.
const DEFAULT_SERVER_URL: &str = match option_env!("RD_SERVER_URL") {
    Some(url) => url,
    None => "ws://127.0.0.1:8080/ws",
};
const SERVER_URL_FILE: &str = "server-url.txt";

/// App logo, embedded into the binary. Regenerate with `assets/generate_icon.py`.
const LOGO_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn logo_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(LOGO_PNG).expect("embedded logo is a valid PNG")
}

mod palette {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(11, 17, 32);
    pub const SURFACE: Color32 = Color32::from_rgb(16, 23, 40);
    pub const CARD: Color32 = Color32::from_rgb(21, 30, 50);
    pub const INPUT_BG: Color32 = Color32::from_rgb(12, 18, 34);
    pub const BORDER: Color32 = Color32::from_rgb(38, 50, 74);
    pub const TEXT: Color32 = Color32::from_rgb(229, 233, 240);
    pub const MUTED: Color32 = Color32::from_rgb(139, 151, 173);
    pub const ACCENT: Color32 = Color32::from_rgb(99, 102, 241);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(165, 168, 255);
    pub const SUCCESS: Color32 = Color32::from_rgb(34, 197, 94);
    pub const WARNING: Color32 = Color32::from_rgb(245, 158, 11);
    pub const IDLE: Color32 = Color32::from_rgb(148, 163, 184);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_icon(logo_icon())
            .with_inner_size([900.0, 580.0])
            .with_min_inner_size([720.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |creation_context| {
            apply_theme(&creation_context.egui_ctx);
            Ok(Box::new(RemoteAccessApp::new(&creation_context.egui_ctx)))
        }),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State of the link to the rendezvous server.
enum Link {
    Connecting,
    Online,
    Offline { reason: String, retrying: bool },
}

/// A connection request this device sent and is waiting on.
struct Outgoing {
    target: Uuid,
    /// Known once the server confirms delivery.
    session_id: Option<Uuid>,
}

struct Incoming {
    session_id: Uuid,
    from_id: Uuid,
}

struct ActiveSession {
    session_id: Uuid,
    peer_id: Uuid,
    role: Role,
}

struct RemoteAccessApp {
    /// This installation's own ID, derived from its secret. Never edited by the UI.
    device_id: String,
    /// The partner ID typed into the "Connect" card.
    remote_id: String,
    /// Shown as a banner when the ID could not be loaded or saved.
    warning: Option<String>,
    copied_at: Option<f64>,
    logo: egui::TextureHandle,

    server_url: String,
    net: net::NetHandle,
    link: Link,
    outgoing: Option<Outgoing>,
    incoming: Vec<Incoming>,
    sessions: Vec<ActiveSession>,
    /// Latest result shown under the connect button.
    notice: Option<(String, Color32)>,
}

impl RemoteAccessApp {
    fn new(ctx: &egui::Context) -> Self {
        let icon = logo_icon();
        let logo = ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            egui::TextureOptions::LINEAR,
        );

        let data_dir = data_directory().ok();
        let (secret, warning) = match data_dir
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no data directory"))
            .and_then(load_or_create_device_secret)
        {
            Ok(secret) => (secret, None),
            Err(error) => (
                new_device_secret(),
                Some(format!(
                    "Could not save this device's ID ({error}). A temporary ID is being used and will change next launch."
                )),
            ),
        };
        let device_id = device_id_from_secret(&secret)
            .expect("generated secrets are always valid")
            .to_string();
        let server_url = server_url(data_dir.as_deref());
        let net = net::spawn(server_url.clone(), secret, ctx.clone());

        Self {
            device_id,
            remote_id: String::new(),
            warning,
            copied_at: None,
            logo,
            server_url,
            net,
            link: Link::Connecting,
            outgoing: None,
            incoming: Vec::new(),
            sessions: Vec::new(),
            notice: None,
        }
    }
}

fn data_directory() -> io::Result<PathBuf> {
    if let Some(app_data) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(app_data).join("Delickate").join("RemoteAccess"));
    }
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("delickate-remote-access"));
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "neither APPDATA nor HOME is set",
    ))
}

/// Server URL: the `RD_SERVER_URL` environment variable, else the first line
/// of `server-url.txt` in the data directory, else the built-in default.
fn server_url(data_dir: Option<&Path>) -> String {
    let non_empty = |value: String| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    };
    env::var("RD_SERVER_URL")
        .ok()
        .and_then(non_empty)
        .or_else(|| {
            data_dir
                .and_then(|dir| fs::read_to_string(dir.join(SERVER_URL_FILE)).ok())
                .and_then(|text| text.lines().next().map(str::to_owned))
                .and_then(non_empty)
        })
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_owned())
}

/// 32 bytes from the OS random generator (via two v4 UUIDs, 244 random bits), hex encoded.
fn new_device_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Reads this installation's private secret from `directory`, creating one if
/// it is missing. A corrupt file is preserved as `device-secret.invalid` and
/// replaced, instead of preventing the app from starting.
fn load_or_create_device_secret(directory: &Path) -> io::Result<String> {
    fs::create_dir_all(directory)?;
    let secret_path = directory.join("device-secret");

    match fs::read_to_string(&secret_path) {
        Ok(contents) => {
            let secret = contents.trim();
            if device_id_from_secret(secret).is_some() {
                return Ok(secret.to_owned());
            }
            fs::rename(&secret_path, directory.join("device-secret.invalid"))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let secret = new_device_secret();
    fs::write(&secret_path, &secret)?;
    Ok(secret)
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteIdStatus {
    Empty,
    Invalid,
    OwnDevice,
    Valid(Uuid),
}

fn classify_remote_id(input: &str, own_id: &str) -> RemoteIdStatus {
    let input = input.trim();
    if input.is_empty() {
        return RemoteIdStatus::Empty;
    }
    match Uuid::parse_str(input) {
        Err(_) => RemoteIdStatus::Invalid,
        Ok(id) if id.to_string() == own_id => RemoteIdStatus::OwnDevice,
        Ok(id) => RemoteIdStatus::Valid(id),
    }
}

fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_owned()
}

// ---------------------------------------------------------------------------
// Server events
// ---------------------------------------------------------------------------

impl RemoteAccessApp {
    fn process_network_events(&mut self) {
        while let Ok(event) = self.net.events.try_recv() {
            match event {
                net::NetEvent::Connecting => {
                    if !matches!(self.link, Link::Offline { .. }) {
                        self.link = Link::Connecting;
                    }
                }
                net::NetEvent::Disconnected { reason, retrying } => {
                    // The server ends every session of a disconnected device.
                    self.outgoing = None;
                    self.incoming.clear();
                    self.sessions.clear();
                    self.link = Link::Offline { reason, retrying };
                }
                net::NetEvent::Message(message) => self.handle_server_message(message),
            }
        }
    }

    fn handle_server_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Registered { device_id, .. } => {
                self.link = Link::Online;
                if device_id.to_string() != self.device_id {
                    self.warning = Some("The server reported a different device ID than this app shows.".into());
                }
            }
            ServerMessage::RequestSent { session_id, target_id } => {
                match &mut self.outgoing {
                    Some(outgoing) if outgoing.target == target_id && outgoing.session_id.is_none() => {
                        outgoing.session_id = Some(session_id);
                    }
                    // Cancelled before the server confirmed it: withdraw it now.
                    _ => self.net.send(ClientMessage::EndSession { session_id }),
                }
            }
            ServerMessage::IncomingRequest { session_id, from_id } => {
                self.incoming.push(Incoming { session_id, from_id });
            }
            ServerMessage::SessionStarted { session_id, peer_id, role } => {
                if role == Role::Viewer {
                    self.outgoing = None;
                    self.notice = Some((
                        format!("Connected to {}.", short_id(peer_id)),
                        palette::SUCCESS,
                    ));
                }
                self.sessions.push(ActiveSession { session_id, peer_id, role });
            }
            ServerMessage::SessionRejected { session_id } => {
                if self.outgoing_is(session_id) {
                    self.outgoing = None;
                    self.notice = Some(("The other person declined the request.".into(), palette::WARNING));
                }
            }
            ServerMessage::SessionEnded { session_id, reason } => {
                let text = match reason {
                    EndReason::PeerEnded => "The other side ended the session.",
                    EndReason::PeerDisconnected => "The other device went offline.",
                    EndReason::Timeout => "The request expired without an answer.",
                };
                let was_relevant = self.outgoing_is(session_id)
                    || self.sessions.iter().any(|s| s.session_id == session_id);
                if self.outgoing_is(session_id) {
                    self.outgoing = None;
                }
                self.incoming.retain(|r| r.session_id != session_id);
                self.sessions.retain(|s| s.session_id != session_id);
                if was_relevant {
                    self.notice = Some((text.into(), palette::WARNING));
                }
            }
            ServerMessage::Error { message, session_id, .. } => {
                // Errors about a request that never got a session ID answer our pending request.
                if self
                    .outgoing
                    .as_ref()
                    .is_some_and(|o| o.session_id.is_none() || o.session_id == session_id)
                {
                    self.outgoing = None;
                }
                self.notice = Some((message, palette::WARNING));
            }
            // Screen and input traffic arrives here in the next step.
            ServerMessage::Relay { .. } | ServerMessage::Pong => {}
        }
    }

    fn outgoing_is(&self, session_id: Uuid) -> bool {
        self.outgoing
            .as_ref()
            .is_some_and(|o| o.session_id == Some(session_id))
    }

    fn request_connection(&mut self, target: Uuid) {
        self.notice = None;
        self.outgoing = Some(Outgoing { target, session_id: None });
        self.net.send(ClientMessage::ConnectRequest { target_id: target });
    }

    fn cancel_request(&mut self) {
        if let Some(outgoing) = self.outgoing.take()
            && let Some(session_id) = outgoing.session_id
        {
            self.net.send(ClientMessage::EndSession { session_id });
        }
        self.notice = Some(("Request cancelled.".into(), palette::MUTED));
    }

    fn answer_request(&mut self, session_id: Uuid, accept: bool) {
        self.incoming.retain(|r| r.session_id != session_id);
        self.net.send(ClientMessage::ConnectResponse { session_id, accept });
    }

    fn end_session(&mut self, session_id: Uuid) {
        self.sessions.retain(|s| s.session_id != session_id);
        self.net.send(ClientMessage::EndSession { session_id });
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let visuals = &mut style.visuals;
        *visuals = egui::Visuals::dark();
        visuals.panel_fill = palette::BG;
        visuals.window_fill = palette::CARD;
        visuals.faint_bg_color = palette::CARD;
        visuals.extreme_bg_color = palette::INPUT_BG;
        visuals.hyperlink_color = palette::ACCENT_SOFT;
        visuals.selection.bg_fill = palette::ACCENT.gamma_multiply(0.6);
        visuals.selection.stroke = Stroke::new(1.5_f32, palette::ACCENT);

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette::TEXT);
        widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette::BORDER);

        widgets.inactive.weak_bg_fill = Color32::from_rgb(34, 45, 68);
        widgets.inactive.bg_fill = Color32::from_rgb(34, 45, 68);
        widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette::BORDER);
        widgets.inactive.fg_stroke = Stroke::new(1.0_f32, palette::TEXT);

        widgets.hovered.weak_bg_fill = Color32::from_rgb(45, 58, 86);
        widgets.hovered.bg_fill = Color32::from_rgb(45, 58, 86);
        widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette::ACCENT);
        widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

        widgets.active.weak_bg_fill = palette::ACCENT;
        widgets.active.bg_fill = palette::ACCENT;
        widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.corner_radius = CornerRadius::same(8);
        }

        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(14.0)),
        ]
        .into();

        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
    });
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl eframe::App for RemoteAccessApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_network_events();
        let now = ctx.input(|input| input.time);

        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(palette::SURFACE)
                    .inner_margin(Margin::symmetric(24, 14))
                    .stroke(Stroke::new(1.0_f32, palette::BORDER)),
            )
            .show(ctx, |ui| header(ui, &self.logo));

        egui::TopBottomPanel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(palette::SURFACE)
                    .inner_margin(Margin::symmetric(24, 8))
                    .stroke(Stroke::new(1.0_f32, palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(rd_core::version()).small().color(palette::MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Server: {}", self.server_url))
                                .small()
                                .color(palette::MUTED),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette::BG)
                    .inner_margin(Margin::same(24)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(warning) = &self.warning {
                        banner(ui, warning);
                        ui.add_space(12.0);
                    }
                    if let Link::Offline { reason, retrying } = &self.link {
                        let suffix = if *retrying { " Retrying…" } else { "" };
                        banner(ui, &format!("Not connected to the server. {reason}{suffix}"));
                        ui.add_space(12.0);
                    }

                    ui.columns(2, |columns| {
                        self.connect_card(&mut columns[0]);
                        self.device_card(&mut columns[1], now);
                    });

                    if !self.sessions.is_empty() {
                        ui.add_space(16.0);
                        self.sessions_card(ui);
                    }
                });
            });

        self.approval_prompt(ctx);

        if let Some(copied_at) = self.copied_at {
            let remaining = COPY_FEEDBACK_SECS - (now - copied_at);
            if remaining > 0.0 {
                ctx.request_repaint_after(Duration::from_secs_f64(remaining));
            } else {
                self.copied_at = None;
            }
        }
    }
}

impl RemoteAccessApp {
    fn connect_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            card_title(
                ui,
                "CONNECT",
                "Connect to a device",
                "Enter the device ID shared by the person you're helping.",
            );

            ui.add_space(6.0);
            ui.label(RichText::new("Partner device ID").strong());
            let waiting = self.outgoing.is_some();
            ui.add_enabled(
                !waiting,
                egui::TextEdit::singleline(&mut self.remote_id)
                    .hint_text("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
                    .font(TextStyle::Monospace)
                    .margin(Margin::symmetric(10, 9))
                    .desired_width(f32::INFINITY),
            );

            let status = classify_remote_id(&self.remote_id, &self.device_id);
            let (message, color) = match status {
                RemoteIdStatus::Empty => ("Paste an ID to get started.", palette::MUTED),
                RemoteIdStatus::Invalid => {
                    ("That doesn't look like a valid device ID.", palette::WARNING)
                }
                RemoteIdStatus::OwnDevice => ("That's this device's own ID.", palette::WARNING),
                RemoteIdStatus::Valid(_) => ("ID format looks good.", palette::SUCCESS),
            };
            ui.label(RichText::new(message).small().color(color));

            ui.add_space(6.0);
            let width = ui.available_width();
            let online = matches!(self.link, Link::Online);

            if waiting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Waiting for the other person to accept…").color(palette::TEXT),
                    );
                });
                if ui
                    .add(egui::Button::new("Cancel request").min_size(Vec2::new(width, 36.0)))
                    .clicked()
                {
                    self.cancel_request();
                }
            } else {
                let target = match status {
                    RemoteIdStatus::Valid(id) => Some(id),
                    _ => None,
                };
                let clicked = ui
                    .add_enabled(
                        online && target.is_some(),
                        egui::Button::new(
                            RichText::new("Request connection").strong().color(Color32::WHITE),
                        )
                        .fill(palette::ACCENT)
                        .min_size(Vec2::new(width, 40.0)),
                    )
                    .clicked();
                if clicked && let Some(target) = target {
                    self.request_connection(target);
                }
            }

            ui.add_space(4.0);
            if let Some((text, color)) = &self.notice {
                ui.label(RichText::new(text).small().color(*color));
            } else if !online {
                note(ui, "Waiting for the server connection…");
            } else {
                note(ui, "The other person must accept before anything is shared.");
            }
        });
    }

    fn device_card(&mut self, ui: &mut egui::Ui, now: f64) {
        card(ui, |ui| {
            card_title(
                ui,
                "THIS DEVICE",
                "Your device ID",
                "Share this ID with the person who will connect to you.",
            );

            ui.add_space(6.0);
            egui::Frame::new()
                .fill(palette::INPUT_BG)
                .stroke(Stroke::new(1.0_f32, palette::BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(&self.device_id)
                                .monospace()
                                .size(15.0)
                                .color(palette::ACCENT_SOFT),
                        )
                        .wrap(),
                    );
                });

            let copied = self
                .copied_at
                .is_some_and(|at| now - at < COPY_FEEDBACK_SECS);
            let label = if copied { "Copied!" } else { "Copy ID" };
            let width = ui.available_width();
            if ui
                .add(egui::Button::new(label).min_size(Vec2::new(width, 36.0)))
                .clicked()
            {
                ui.ctx().copy_text(self.device_id.clone());
                self.copied_at = Some(now);
            }

            ui.add_space(4.0);
            let hosting = self.sessions.iter().filter(|s| s.role == Role::Host).count();
            let (status, color) = match &self.link {
                _ if hosting > 0 => (format!("Connected ({hosting})"), palette::SUCCESS),
                Link::Online => ("Ready".to_owned(), palette::SUCCESS),
                Link::Connecting => ("Connecting…".to_owned(), palette::WARNING),
                Link::Offline { .. } => ("Offline".to_owned(), palette::IDLE),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new("Status").color(palette::MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    status_pill(ui, &status, color);
                });
            });

            ui.add_space(4.0);
            note(
                ui,
                "Every incoming request asks for your approval first. Unattended access is not enabled.",
            );
        });
    }

    fn sessions_card(&mut self, ui: &mut egui::Ui) {
        let mut to_end = None;
        card_with_height(ui, 0.0, |ui| {
            ui.label(RichText::new("SESSIONS").small().strong().color(palette::ACCENT_SOFT));
            for session in &self.sessions {
                ui.horizontal(|ui| {
                    let text = match session.role {
                        Role::Viewer => format!("You are connected to {}", session.peer_id),
                        Role::Host => format!("{} is connected to this device", session.peer_id),
                    };
                    ui.label(RichText::new(text).color(palette::TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("End").clicked() {
                            to_end = Some(session.session_id);
                        }
                    });
                });
            }
            note(ui, "Screen sharing and remote control are coming in the next step.");
        });
        if let Some(session_id) = to_end {
            self.end_session(session_id);
        }
    }

    /// Asks the user to approve the oldest pending incoming request.
    fn approval_prompt(&mut self, ctx: &egui::Context) {
        let Some(request) = self.incoming.first() else {
            return;
        };
        let (session_id, from_id) = (request.session_id, request.from_id);
        let mut answer = None;

        egui::Modal::new(egui::Id::new(("approval", session_id))).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("CONNECTION REQUEST").small().strong().color(palette::WARNING));
            ui.label(RichText::new("Someone wants to connect to this PC").heading().strong());
            ui.add_space(4.0);
            ui.label(RichText::new("Requesting device:").color(palette::MUTED));
            ui.label(RichText::new(from_id.to_string()).monospace().color(palette::ACCENT_SOFT));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Only allow this if you know who it is and you expected their request. \
                     You can end the session at any time.",
                )
                .color(palette::TEXT),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Allow").strong().color(Color32::WHITE))
                            .fill(palette::ACCENT),
                    )
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui.button("Deny").clicked() {
                    answer = Some(false);
                }
            });
        });

        if let Some(accept) = answer {
            self.answer_request(session_id, accept);
        }
    }
}

fn header(ui: &mut egui::Ui, logo: &egui::TextureHandle) {
    ui.horizontal(|ui| {
        ui.add(egui::Image::from_texture(logo).fit_to_exact_size(Vec2::splat(42.0)));

        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(APP_TITLE)
                    .size(18.0)
                    .strong()
                    .color(palette::TEXT),
            );
            ui.label(
                RichText::new("Remote support for devices you own or are authorized to access.")
                    .small()
                    .color(palette::MUTED),
            );
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            status_pill(ui, "Preview", palette::ACCENT_SOFT);
        });
    });
}

fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    card_with_height(ui, 300.0, add_contents);
}

fn card_with_height(ui: &mut egui::Ui, min_height: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette::CARD)
        .stroke(Stroke::new(1.0_f32, palette::BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(22))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(min_height);
            add_contents(ui);
        });
}

fn card_title(ui: &mut egui::Ui, eyebrow: &str, title: &str, subtitle: &str) {
    ui.label(
        RichText::new(eyebrow)
            .small()
            .strong()
            .color(palette::ACCENT_SOFT),
    );
    ui.label(RichText::new(title).heading().strong().color(palette::TEXT));
    ui.label(RichText::new(subtitle).color(palette::MUTED));
}

fn status_pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let dot = |ui: &mut egui::Ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                };
                let label = RichText::new(text).small().strong().color(color);
                // `horizontal` inherits the parent's direction; keep the dot
                // visually before the text either way.
                if ui.layout().prefer_right_to_left() {
                    ui.label(label);
                    dot(ui);
                } else {
                    dot(ui);
                    ui.label(label);
                }
            });
        });
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(palette::MUTED));
}

fn banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(palette::WARNING.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0_f32, palette::WARNING.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(palette::WARNING));
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        env::temp_dir().join(format!("rd-client-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn device_secret_is_created_then_reused() {
        let dir = temp_dir();
        let first = load_or_create_device_secret(&dir).unwrap();
        let second = load_or_create_device_secret(&dir).unwrap();
        assert_eq!(first, second);
        assert!(device_id_from_secret(&first).is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_device_secret_is_replaced_and_backed_up() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("device-secret"), "not-a-secret").unwrap();

        let secret = load_or_create_device_secret(&dir).unwrap();
        assert!(device_id_from_secret(&secret).is_some());
        assert_eq!(
            fs::read_to_string(dir.join("device-secret.invalid")).unwrap(),
            "not-a-secret"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_url_prefers_file_over_default() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        if env::var_os("RD_SERVER_URL").is_none() {
            assert_eq!(server_url(Some(&dir)), DEFAULT_SERVER_URL);
            fs::write(dir.join(SERVER_URL_FILE), "  wss://example.test/ws  
# note
").unwrap();
            assert_eq!(server_url(Some(&dir)), "wss://example.test/ws");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remote_id_is_classified() {
        let own = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();

        assert_eq!(classify_remote_id("  ", &own), RemoteIdStatus::Empty);
        assert_eq!(classify_remote_id("abc", &own), RemoteIdStatus::Invalid);
        assert_eq!(classify_remote_id(&own, &own), RemoteIdStatus::OwnDevice);
        assert_eq!(
            classify_remote_id(&own.to_uppercase(), &own),
            RemoteIdStatus::OwnDevice
        );
        assert_eq!(
            classify_remote_id(&other, &own),
            RemoteIdStatus::Valid(Uuid::parse_str(&other).unwrap())
        );
    }
}
