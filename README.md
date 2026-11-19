# Delickate Remote Access

A Rust workspace for a consent-based remote-access application.

The Windows desktop app connects to a rendezvous server (`nat-server`, a
separate project). Users can request a connection to another device by ID; the
other person must click **Allow** before a session starts. Screen sharing and
remote input are not implemented yet.

On first launch the app creates a private secret under
`%APPDATA%\Delickate\RemoteAccess\device-secret`. The device ID shown in the app
is derived from it; this is the ID to share.

## Run

```text
cargo run -p rd-client
```

## Server address

Resolved in this order:

1. `RD_SERVER_URL` environment variable when the app starts
2. First line of `%APPDATA%\Delickate\RemoteAccess\server-url.txt`
3. `RD_SERVER_URL` set at **build** time, e.g.
   `$env:RD_SERVER_URL="wss://remote.example.com/ws"; cargo build --release -p rd-client`
4. `ws://127.0.0.1:8080/ws` (local development)

See the nat-server README for deploying the server.

The shared protocol and identity code live in `crates/rd-core`.
