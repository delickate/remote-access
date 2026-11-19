//! Wire protocol between the desktop app and this server.
//!
//! Control messages are JSON text frames tagged by `type`. Bulk data (screen
//! frames later on) travels as binary frames whose first 16 bytes are the
//! session ID; the server forwards those unchanged to the other participant.
//!
//! Keep this file in sync with `src/protocol.rs` in the nat-server repository.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;

/// Length of the session-ID prefix on binary frames.
pub const BINARY_HEADER_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Must be the first message on every connection. The device ID is
    /// derived from the secret, so a device cannot claim someone else's ID.
    Register {
        protocol_version: u16,
        client_version: String,
        /// 32 random bytes, hex encoded. Generated once per installation.
        device_secret: String,
    },
    /// Ask the owner of `target_id` for a session.
    ConnectRequest { target_id: Uuid },
    /// Sent by the target to approve or deny a pending request.
    ConnectResponse { session_id: Uuid, accept: bool },
    /// Cancel a pending request or end an active session.
    EndSession { session_id: Uuid },
    /// Forwarded as-is to the other participant of an active session.
    Relay {
        session_id: Uuid,
        payload: serde_json::Value,
    },
    Ping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Registered {
        device_id: Uuid,
        server_version: String,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
    },
    /// Your request was delivered and is waiting for the other side.
    RequestSent { session_id: Uuid, target_id: Uuid },
    /// Someone wants to connect to you. Answer with `connect_response`.
    IncomingRequest { session_id: Uuid, from_id: Uuid },
    /// The request was accepted; relay traffic may now flow.
    SessionStarted {
        session_id: Uuid,
        peer_id: Uuid,
        role: Role,
    },
    SessionRejected { session_id: Uuid },
    SessionEnded { session_id: Uuid, reason: EndReason },
    Relay {
        session_id: Uuid,
        payload: serde_json::Value,
    },
    Pong,
}

impl ServerMessage {
    pub fn error(code: ErrorCode, session_id: Option<Uuid>) -> Self {
        Self::Error {
            code,
            message: code.description().to_owned(),
            session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The side that asked to connect and will view the screen.
    Viewer,
    /// The side that approved and will share its screen.
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    PeerEnded,
    PeerDisconnected,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    InvalidSecret,
    NotRegistered,
    AlreadyRegistered,
    MalformedMessage,
    TargetOffline,
    SelfConnect,
    RateLimited,
    TooManyRequests,
    TooManySessions,
    UnknownSession,
    Replaced,
}

impl ErrorCode {
    pub fn description(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "This app version is not supported by the server. Please update.",
            Self::InvalidSecret => "The device secret is invalid.",
            Self::NotRegistered => "The first message must be `register`.",
            Self::AlreadyRegistered => "This connection is already registered.",
            Self::MalformedMessage => "The message could not be understood.",
            Self::TargetOffline => "That device is offline or the ID is wrong.",
            Self::SelfConnect => "You cannot connect to your own device.",
            Self::RateLimited => "Too many connection requests. Wait a minute and try again.",
            Self::TooManyRequests => "Too many pending requests. Wait for an answer first.",
            Self::TooManySessions => "Too many active sessions.",
            Self::UnknownSession => "That session does not exist or is not active.",
            Self::Replaced => "This device signed in again from another connection.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_use_snake_case_tags() {
        let id = Uuid::nil();
        let json = serde_json::to_string(&ClientMessage::ConnectRequest { target_id: id }).unwrap();
        assert_eq!(
            json,
            r#"{"type":"connect_request","target_id":"00000000-0000-0000-0000-000000000000"}"#
        );
        assert_eq!(serde_json::to_string(&ClientMessage::Ping).unwrap(), r#"{"type":"ping"}"#);
    }

    #[test]
    fn error_omits_missing_session_id() {
        let json = serde_json::to_value(ServerMessage::error(ErrorCode::TargetOffline, None)).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "target_offline");
        assert!(json.get("session_id").is_none());
    }

    #[test]
    fn unknown_types_are_rejected() {
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"nope"}"#).is_err());
    }
}
