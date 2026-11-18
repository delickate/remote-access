// Hide the extra console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke,
    TextStyle, Vec2,
};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const APP_TITLE: &str = "Delickate Remote Access";
const COPY_FEEDBACK_SECS: f64 = 1.5;

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

/// App logo, embedded into the binary. Regenerate with `assets/generate_icon.py`.
const LOGO_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn logo_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(LOGO_PNG).expect("embedded logo is a valid PNG")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        Box::new(|creation_context| {
            apply_theme(&creation_context.egui_ctx);
            Ok(Box::new(RemoteAccessApp::new(&creation_context.egui_ctx)))
        }),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct RemoteAccessApp {
    /// This installation's own ID. Never edited by the UI.
    device_id: String,
    /// The partner ID typed into the "Connect" card.
    remote_id: String,
    /// Shown as a banner when the ID could not be loaded or saved.
    warning: Option<String>,
    copied_at: Option<f64>,
    logo: egui::TextureHandle,
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

        let (device_id, warning) = match data_directory().and_then(|dir| load_or_create_device_id(&dir)) {
            Ok(id) => (id, None),
            Err(error) => (
                uuid::Uuid::new_v4().to_string(),
                Some(format!(
                    "Could not save this device's ID ({error}). A temporary ID is being used and will change next launch."
                )),
            ),
        };

        Self {
            device_id,
            remote_id: String::new(),
            warning,
            copied_at: None,
            logo,
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

/// Reads the persisted device ID from `directory`, creating one if it is
/// missing. A corrupt file is preserved as `device-id.invalid` and replaced,
/// instead of preventing the app from starting.
fn load_or_create_device_id(directory: &Path) -> io::Result<String> {
    fs::create_dir_all(directory)?;
    let id_path = directory.join("device-id");

    match fs::read_to_string(&id_path) {
        Ok(contents) => {
            if let Ok(id) = uuid::Uuid::parse_str(contents.trim()) {
                return Ok(id.to_string());
            }
            fs::rename(&id_path, directory.join("device-id.invalid"))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let device_id = uuid::Uuid::new_v4().to_string();
    fs::write(&id_path, &device_id)?;
    Ok(device_id)
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteIdStatus {
    Empty,
    Invalid,
    OwnDevice,
    Valid,
}

fn classify_remote_id(input: &str, own_id: &str) -> RemoteIdStatus {
    let input = input.trim();
    if input.is_empty() {
        return RemoteIdStatus::Empty;
    }
    match uuid::Uuid::parse_str(input) {
        Err(_) => RemoteIdStatus::Invalid,
        Ok(id) if id.to_string() == own_id => RemoteIdStatus::OwnDevice,
        Ok(_) => RemoteIdStatus::Valid,
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
                            RichText::new("Windows desktop preview")
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
                if let Some(warning) = &self.warning {
                    banner(ui, warning);
                    ui.add_space(12.0);
                }

                ui.columns(2, |columns| {
                    self.connect_card(&mut columns[0]);
                    self.device_card(&mut columns[1], now);
                });
            });

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
            ui.add(
                egui::TextEdit::singleline(&mut self.remote_id)
                    .hint_text("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
                    .font(TextStyle::Monospace)
                    .margin(Margin::symmetric(10, 9))
                    .desired_width(f32::INFINITY),
            );

            let (message, color) = match classify_remote_id(&self.remote_id, &self.device_id) {
                RemoteIdStatus::Empty => ("Paste an ID to get started.", palette::MUTED),
                RemoteIdStatus::Invalid => {
                    ("That doesn't look like a valid device ID.", palette::WARNING)
                }
                RemoteIdStatus::OwnDevice => ("That's this device's own ID.", palette::WARNING),
                RemoteIdStatus::Valid => ("ID format looks good.", palette::SUCCESS),
            };
            ui.label(RichText::new(message).small().color(color));

            ui.add_space(6.0);
            let width = ui.available_width();
            ui.add_enabled(
                false,
                egui::Button::new(RichText::new("Request connection").strong().color(Color32::WHITE))
                    .fill(palette::ACCENT)
                    .min_size(Vec2::new(width, 40.0)),
            );

            ui.add_space(4.0);
            note(
                ui,
                "Connections are unavailable until networking and identity checks are implemented.",
            );
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Status").color(palette::MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    status_pill(ui, "Not sharing", palette::IDLE);
                });
            });

            ui.add_space(4.0);
            note(
                ui,
                "This ID is not connectable yet. Incoming sessions will require your explicit \
                 approval; unattended access is not enabled.",
            );
        });
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
    egui::Frame::new()
        .fill(palette::CARD)
        .stroke(Stroke::new(1.0_f32, palette::BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(22))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(300.0);
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
    fn device_id_is_created_then_reused() {
        let dir = temp_dir();
        let first = load_or_create_device_id(&dir).unwrap();
        let second = load_or_create_device_id(&dir).unwrap();
        assert_eq!(first, second);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_device_id_is_replaced_and_backed_up() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("device-id"), "not-a-uuid").unwrap();

        let id = load_or_create_device_id(&dir).unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(
            fs::read_to_string(dir.join("device-id.invalid")).unwrap(),
            "not-a-uuid"
        );
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
        assert_eq!(classify_remote_id(&other, &own), RemoteIdStatus::Valid);
    }
}
