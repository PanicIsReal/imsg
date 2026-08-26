//! imsg-bridge/v1 wire protocol types.

pub mod event;

pub use event::{BridgeEvent, ContactsState};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const PROTOCOL_VERSION: &str = "imsg-bridge/v1";

/// Methods allowed on the bridge (read-mostly default).
pub const ALLOWED_METHODS: &[&str] = &[
    "status",
    "chats.list",
    "messages.history",
    "messages.after",
    "messages.search",
    "watch.ack",
    "attachments.fetch",
    "handles.check",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Envelope {
    Req {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Res {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
    Event {
        topic: String,
        payload: Value,
    },
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("method not allowed: {0}")]
    MethodNotAllowed(String),
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),
}

impl Envelope {
    pub fn parse_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn method_allowed(method: &str) -> bool {
        ALLOWED_METHODS.contains(&method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_request() {
        let env = Envelope::Req {
            id: "1".into(),
            method: "chats.list".into(),
            params: serde_json::json!({"limit": 50}),
        };
        let line = env.to_line().unwrap();
        let parsed = Envelope::parse_line(&line).unwrap();
        assert_eq!(env, parsed);
    }

    #[test]
    fn allowlist_blocks_send() {
        assert!(!Envelope::method_allowed("send"));
        assert!(Envelope::method_allowed("status"));
        assert!(!Envelope::method_allowed("contacts.authorize"));
    }
}
