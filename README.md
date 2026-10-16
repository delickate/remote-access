# Delickate Remote Access

A Rust workspace for a consent-based remote-access application.

The current Windows desktop preview is an initial UI shell. It does not yet
establish connections, capture screens, or send input. Connection controls stay
disabled until networking, identity checks, and explicit session approval are
implemented.

The app creates a persistent UUID for this installation under the Windows
`%APPDATA%\Delickate\RemoteAccess` directory and displays it as the device ID to
share. The ID is not currently usable to establish a connection.

## Run

```text
cargo run -p rd-client
```

The shared protocol and core code live in `crates/rd-core`.
