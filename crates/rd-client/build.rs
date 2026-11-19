// Embeds the app icon and version details into the Windows .exe, so the file
// shows the logo in Explorer, on the desktop and in the taskbar.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("ProductName", "Delickate Remote Access")
            .set("FileDescription", "Delickate Remote Access")
            .set("OriginalFilename", "DelickateRemoteAccess.exe");
        resource
            .compile()
            .expect("failed to embed Windows resources (icon)");
    }
}
