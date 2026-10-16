use eframe::egui;
use std::{env, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = RemoteAccessApp::load().map_err(|error| {
        eprintln!("Could not load this device's ID: {error}");
        error
    })?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 500.0])
            .with_min_inner_size([620.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Delickate Remote Access",
        options,
        Box::new(move |_creation_context| Ok(Box::new(app))),
    )?;
    Ok(())
}

struct RemoteAccessApp {
    device_id: String,
}

impl RemoteAccessApp {
    fn load() -> io::Result<Self> {
        let app_data = env::var_os("APPDATA").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "APPDATA environment variable is not set")
        })?;
        let app_directory = PathBuf::from(app_data)
            .join("Delickate")
            .join("RemoteAccess");
        fs::create_dir_all(&app_directory)?;

        let id_path = app_directory.join("device-id");
        let device_id = match fs::read_to_string(&id_path) {
            Ok(device_id) => {
                uuid::Uuid::parse_str(device_id.trim()).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("stored device ID is invalid: {error}"),
                    )
                })?;
                device_id.trim().to_owned()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let device_id = uuid::Uuid::new_v4().to_string();
                fs::write(id_path, &device_id)?;
                device_id
            }
            Err(error) => return Err(error),
        };

        Ok(Self { device_id })
    }
}

impl eframe::App for RemoteAccessApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("Delickate Remote Access");
            ui.label("Remote support for devices you own or are authorized to access.");
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.heading("Connect to a device");
                    ui.add_space(8.0);
                    ui.label("Device ID");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.device_id)
                            .hint_text("Enter the other device's ID")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(8.0);
                    ui.add_enabled(false, egui::Button::new("Request connection"));
                    ui.add_space(4.0);
                    ui.small("Connections are unavailable until networking and identity checks are implemented.");
                });

                columns[1].group(|ui| {
                    ui.heading("This device");
                    ui.add_space(8.0);
                    ui.label("Your device ID — share this with the person connecting:");
                    ui.horizontal(|ui| {
                        ui.monospace(&self.device_id);
                        if ui.button("Copy ID").clicked() {
                            ui.ctx().copy_text(self.device_id.clone());
                        }
                    });
                    ui.add_space(8.0);
                    ui.label("Status: Not sharing");
                    ui.add_space(8.0);
                    ui.label(
                        "This ID is not connectable yet. Incoming sessions will require your \
                         explicit approval; unattended access is not enabled.",
                    );
                });
            });

            ui.add_space(24.0);
            ui.separator();
            ui.add_space(8.0);
            ui.small(format!("{}  |  Windows desktop preview", rd_core::version()));
        });
    }
}