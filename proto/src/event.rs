//! Typed view of events on the two sockets.
//!
//! The Mac bridge speaks `message` / `db.generation` / `contacts` on WSS.
//! Sync speaks `sync.*` on the Unix socket. The same string never means two shapes.

use crate::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Contacts availability as the plugin needs to reason about it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContactsState {
    /// Never asked the Mac (link has not come up yet).
    #[default]
    Unknown,
    /// Mac reachable, names are not available, and asking is possible.
    Unavailable,
    /// A dialog is up on the Mac; the user has not answered.
    Prompting,
    /// Names are flowing.
    Granted,
}

/// Typed view of events the Mac bridge pushes down the WSS.
///
/// `Unknown` keeps the match exhaustive so a future bridge topic cannot
/// silently regress into a discard arm.
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeEvent {
    Message(Value),
    DbGeneration { generation: String },
    Contacts(ContactsState),
    Unknown { topic: String },
}

impl BridgeEvent {
    /// `None` when the envelope is not an `Event`.
    pub fn from_envelope(env: &Envelope) -> Option<Self> {
        let Envelope::Event { topic, payload } = env else {
            return None;
        };
        Some(match topic.as_str() {
            "message" => Self::Message(payload.clone()),
            "db.generation" => Self::DbGeneration {
                generation: payload
                    .get("generation")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            "contacts" => Self::Contacts(contacts_state_from_payload(payload)),
            _ => Self::Unknown {
                topic: topic.clone(),
            },
        })
    }
}

fn contacts_state_from_payload(payload: &Value) -> ContactsState {
    let raw = payload
        .get("state")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    serde_json::from_value(raw).unwrap_or(ContactsState::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_envelope_ignores_non_events() {
        let env = Envelope::Ping;
        assert_eq!(BridgeEvent::from_envelope(&env), None);
    }

    #[test]
    fn from_envelope_maps_known_topics() {
        let message = Envelope::Event {
            topic: "message".into(),
            payload: json!({"id": 9, "chat_id": 1}),
        };
        assert_eq!(
            BridgeEvent::from_envelope(&message),
            Some(BridgeEvent::Message(json!({"id": 9, "chat_id": 1})))
        );

        let gen = Envelope::Event {
            topic: "db.generation".into(),
            payload: json!({"generation": "abc", "at": "now"}),
        };
        assert_eq!(
            BridgeEvent::from_envelope(&gen),
            Some(BridgeEvent::DbGeneration {
                generation: "abc".into()
            })
        );

        let contacts = Envelope::Event {
            topic: "contacts".into(),
            payload: json!({"state": "granted"}),
        };
        assert_eq!(
            BridgeEvent::from_envelope(&contacts),
            Some(BridgeEvent::Contacts(ContactsState::Granted))
        );
    }

    #[test]
    fn from_envelope_unknown_topic_is_named() {
        let env = Envelope::Event {
            topic: "future.thing".into(),
            payload: json!({}),
        };
        assert_eq!(
            BridgeEvent::from_envelope(&env),
            Some(BridgeEvent::Unknown {
                topic: "future.thing".into()
            })
        );
    }
}
