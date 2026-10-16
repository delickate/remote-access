use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    ClientHello {
        protocol_version: u16,
        client_version: String,
    },
    ServerHello {
        protocol_version: u16,
        server_version: String,
    },
    ProtocolError {
        supported_version: u16,
    },
}

impl Message {
    pub fn client_hello(client_version: impl Into<String>) -> Self {
        Self::ClientHello {
            protocol_version: PROTOCOL_VERSION,
            client_version: client_version.into(),
        }
    }

    pub fn server_hello(server_version: impl Into<String>) -> Self {
        Self::ServerHello {
            protocol_version: PROTOCOL_VERSION,
            server_version: server_version.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_uses_current_protocol_version() {
        assert_eq!(
            Message::client_hello("rd-client v0.1.0"),
            Message::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                client_version: "rd-client v0.1.0".to_owned(),
            }
        );
    }

    #[test]
    fn messages_round_trip_as_json() {
        let message = Message::client_hello("rd-client v0.1.0");
        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn unknown_message_types_are_rejected() {
        let result = serde_json::from_str::<Message>(r#"{"type":"unknown"}"#);

        assert!(result.is_err());
    }
}
