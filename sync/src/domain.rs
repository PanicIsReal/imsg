use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatGuid(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageGuid(String);

pub fn attachment_form_guid(chat_guid: &ChatGuid, identifier: &str) -> String {
    if chat_guid.is_group() {
        return chat_guid.as_str().to_string();
    }
    let ident = identifier.trim();
    if ident.is_empty() {
        chat_guid.as_str().to_string()
    } else {
        ident.to_string()
    }
}

impl ChatGuid {
    pub fn parse(raw: impl AsRef<str>) -> Result<Self> {
        let s = raw.as_ref().trim();
        if s.is_empty() {
            bail!("empty chat guid");
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_group(&self) -> bool {
        self.0.contains(";+;")
    }
}

impl MessageGuid {
    pub fn parse(raw: impl AsRef<str>) -> Result<Self> {
        let s = raw.as_ref().trim();
        if s.is_empty() {
            bail!("empty message guid");
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handle {
    pub address: String,
    pub service: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMeta {
    pub guid: String,
    pub mime: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chat {
    pub guid: ChatGuid,
    pub identifier: String,
    pub display_name: Option<String>,
    pub participants: Vec<Handle>,
    pub last_message_at: Option<String>,
    pub unread_count: u32,
    pub is_group: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub guid: MessageGuid,
    pub chat: ChatGuid,
    pub text: String,
    pub created_at: String,
    pub is_from_me: bool,
    pub sender: Option<Handle>,
    pub attachments: Vec<AttachmentMeta>,
}

impl Chat {
    pub fn from_bb(value: &Value) -> Result<Self> {
        let guid = ChatGuid::parse(value["guid"].as_str().unwrap_or(""))?;
        let identifier = value["chatIdentifier"]
            .as_str()
            .or_else(|| value["identifier"].as_str())
            .unwrap_or("")
            .to_string();
        let display_name = value["displayName"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let participants = parse_handles(&value["participants"]);
        let last_message_at = value
            .get("lastMessage")
            .and_then(|m| timestamp_from_bb(m.get("dateCreated")));
        let unread_count = value["unreadCount"].as_u64().unwrap_or(0) as u32;
        let is_group = guid.is_group();
        Ok(Self {
            guid,
            identifier,
            display_name,
            participants,
            last_message_at,
            unread_count,
            is_group,
        })
    }

    pub fn stub(
        guid: ChatGuid,
        handle: Option<Handle>,
        last_message_at: Option<String>,
        unread_count: u32,
    ) -> Self {
        let identifier = handle
            .as_ref()
            .map(|h| h.address.clone())
            .unwrap_or_else(|| guid.as_str().to_string());
        let display_name = handle.as_ref().and_then(|h| h.name.clone());
        let is_group = guid.is_group();
        Self {
            guid,
            identifier,
            display_name,
            participants: handle.into_iter().collect(),
            last_message_at,
            unread_count,
            is_group,
        }
    }

    pub fn stub_from_message(msg: &Message) -> Self {
        Self::stub(
            msg.chat.clone(),
            msg.sender.clone(),
            Some(msg.created_at.clone()).filter(|s| !s.is_empty()),
            if msg.is_from_me { 0 } else { 1 },
        )
    }

    pub fn apply_contacts(&mut self, book: &ContactBook) {
        for handle in &mut self.participants {
            if handle.name.is_none() {
                if let Some(name) = book.lookup(&handle.address) {
                    handle.name = Some(name.to_string());
                }
            }
        }
        if self.display_name.is_none() {
            if let Some(name) = book.lookup(&self.identifier) {
                self.display_name = Some(name.to_string());
            }
        }
    }

    pub fn to_cache_json(&self, id: i64) -> Value {
        let name = self
            .display_name
            .clone()
            .or_else(|| self.participants.iter().find_map(|h| h.name.clone()))
            .unwrap_or_else(|| self.identifier.clone());
        let participants: Vec<String> = self
            .participants
            .iter()
            .map(|h| h.address.clone())
            .collect();
        json!({
            "id": json_id(id),
            "guid": self.guid.as_str(),
            "name": name,
            "contact_name": name,
            "display_name": self.display_name,
            "identifier": self.identifier,
            "participants": participants,
            "last_message_at": self.last_message_at,
            "unread_count": self.unread_count,
            "is_group": self.is_group,
        })
    }
}

impl Message {
    pub fn apply_contacts(&mut self, book: &ContactBook) {
        if let Some(handle) = &mut self.sender {
            if handle.name.is_none() {
                if let Some(name) = book.lookup(&handle.address) {
                    handle.name = Some(name.to_string());
                }
            }
        }
    }

    pub fn from_bb(value: &Value, fallback_chat: Option<&ChatGuid>) -> Result<Self> {
        let guid = MessageGuid::parse(value["guid"].as_str().unwrap_or(""))?;
        let chat = chat_guid_from_message(value, fallback_chat)?;
        let text = value["text"].as_str().unwrap_or("").to_string();
        let created_at = timestamp_from_bb(value.get("dateCreated"))
            .or_else(|| value["created_at"].as_str().map(str::to_string))
            .unwrap_or_default();
        let is_from_me = value["isFromMe"].as_bool().or(value["is_from_me"].as_bool()).unwrap_or(false);
        let sender = parse_handle(value.get("handle")).or_else(|| {
            value["handle"]["address"].as_str().map(|address| Handle {
                address: address.to_string(),
                service: value["handle"]["service"].as_str().map(str::to_string),
                name: None,
            })
        });
        let attachments = value["attachments"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        let g = a["guid"].as_str()?.to_string();
                        Some(AttachmentMeta {
                            guid: g,
                            mime: a["mime"].as_str().or(a["uti"].as_str()).map(str::to_string),
                            name: a["transferName"]
                                .as_str()
                                .or(a["name"].as_str())
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            guid,
            chat,
            text,
            created_at,
            is_from_me,
            sender,
            attachments,
        })
    }

    pub fn to_cache_json(&self, id: i64, chat_id: i64) -> Value {
        json!({
            "id": json_id(id),
            "chat_id": json_id(chat_id),
            "guid": self.guid.as_str(),
            "text": self.text,
            "created_at": self.created_at,
            "is_from_me": self.is_from_me,
            "sender": self.sender.as_ref().map(|h| h.address.clone()),
            "sender_name": self.sender.as_ref().and_then(|h| h.name.clone()),
            "attachments": self.attachments.iter().map(|a| json!({
                "guid": a.guid,
                "mime": a.mime,
                "name": a.name,
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn stable_id(guid: &str) -> i64 {
    let digest = Sha256::digest(guid.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let n = u64::from_le_bytes(bytes) & (i64::MAX as u64);
    if n == 0 {
        1
    } else {
        n as i64
    }
}

pub fn timestamp_from_bb(raw: Option<&Value>) -> Option<String> {
    let n = match raw? {
        Value::Number(num) => num.as_f64()?,
        Value::String(s) => {
            if s.contains('T') {
                return Some(s.clone());
            }
            s.parse::<f64>().ok()?
        }
        _ => return None,
    };
    let millis = bb_to_unix_millis(n)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn bb_to_unix_millis(n: f64) -> Option<i64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    if n > 1_000_000_000_000.0 {
        Some(n as i64)
    } else if n > 10_000_000_000.0 {
        Some((n / 1_000_000.0) as i64)
    } else if n > 1_000_000_000.0 {
        Some((n * 1000.0) as i64)
    } else {
        Some(((n + 978_307_200.0) * 1000.0) as i64)
    }
}

fn chat_guid_from_message(value: &Value, fallback: Option<&ChatGuid>) -> Result<ChatGuid> {
    if let Some(g) = value["chats"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|c| c["guid"].as_str())
    {
        return ChatGuid::parse(g);
    }
    if let Some(g) = value["chatGuid"].as_str() {
        return ChatGuid::parse(g);
    }
    if let Some(fb) = fallback {
        return Ok(fb.clone());
    }
    bail!("message {} has no chat guid", value["guid"].as_str().unwrap_or("?"))
}

fn parse_handles(value: &Value) -> Vec<Handle> {
    value
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| parse_handle(Some(v))).collect())
        .unwrap_or_default()
}

fn parse_handle(value: Option<&Value>) -> Option<Handle> {
    let v = value?;
    if v.is_null() {
        return None;
    }
    let address = v["address"].as_str()?.to_string();
    Some(Handle {
        address,
        service: v["service"].as_str().map(str::to_string),
        name: v["displayName"]
            .as_str()
            .or(v["name"].as_str())
            .or(v["uncanonicalizedId"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

pub fn json_id(id: i64) -> String {
    id.to_string()
}

pub fn parse_json_id(value: &Value) -> Result<i64> {
    if let Some(n) = value.as_i64() {
        return Ok(n);
    }
    if let Some(n) = value.as_u64() {
        return Ok(i64::try_from(n).unwrap_or(0));
    }
    if let Some(s) = value.as_str() {
        return s.parse().context("id");
    }
    anyhow::bail!("id required")
}

pub fn stringify_row_ids(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        for key in ["id", "chat_id"] {
            if let Some(val) = obj.get(key) {
                if let Ok(n) = parse_json_id(val) {
                    obj.insert(key.to_string(), Value::String(json_id(n)));
                }
            }
        }
    }
    value
}

#[derive(Debug, Default, Clone)]
pub struct ContactBook {
    names: std::collections::HashMap<String, String>,
}

impl ContactBook {
    pub fn from_bb(value: &Value) -> Self {
        let mut book = Self::default();
        let Some(arr) = value.as_array() else {
            return book;
        };
        for contact in arr {
            let name = contact_display_name(contact);
            if name.is_empty() {
                continue;
            }
            let phones = contact["phoneNumbers"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let emails = contact["emails"].as_array().cloned().unwrap_or_default();
            for entry in phones.into_iter().chain(emails) {
                let addr = entry
                    .get("address")
                    .and_then(|v| v.as_str())
                    .or_else(|| entry.as_str());
                let Some(addr) = addr else { continue };
                for key in address_keys(addr) {
                    book.names.entry(key).or_insert_with(|| name.clone());
                }
            }
        }
        book
    }

    pub fn lookup(&self, address: &str) -> Option<&str> {
        address_keys(address)
            .into_iter()
            .find_map(|k| self.names.get(&k).map(String::as_str))
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn contact_display_name(contact: &Value) -> String {
    contact["displayName"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let first = contact["firstName"].as_str().unwrap_or("").trim();
            let last = contact["lastName"].as_str().unwrap_or("").trim();
            format!("{first} {last}").trim().to_string()
        })
}

fn address_keys(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut keys = vec![raw.to_lowercase(), digits.clone()];
    if digits.len() == 11 && digits.starts_with('1') {
        keys.push(digits[1..].to_string());
        keys.push(format!("+{digits}"));
        keys.push(format!("+{}", &digits[1..]));
    } else if digits.len() == 10 {
        keys.push(format!("1{digits}"));
        keys.push(format!("+1{digits}"));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_and_group_guids() {
        let dm = ChatGuid::parse("iMessage;-;+15551234567").unwrap();
        assert!(!dm.is_group());
        let group = ChatGuid::parse("iMessage;+;chat123").unwrap();
        assert!(group.is_group());
        assert!(ChatGuid::parse("").is_err());
        assert_eq!(
            attachment_form_guid(&dm, "+15551234567"),
            "+15551234567"
        );
        assert_eq!(
            attachment_form_guid(&ChatGuid::parse("any;-;+17803700650").unwrap(), "+17803700650"),
            "+17803700650"
        );
        assert_eq!(
            attachment_form_guid(&group, "chat123"),
            "iMessage;+;chat123"
        );
    }

    #[test]
    fn stable_id_never_zero_and_repeatable() {
        let a = stable_id("iMessage;-;+15551234567");
        let b = stable_id("iMessage;-;+15551234567");
        assert_eq!(a, b);
        assert_ne!(a, 0);
        assert_ne!(stable_id("other"), a);
    }

    #[test]
    fn hashed_ids_serialize_as_strings() {
        let raw = json!({
            "guid": "iMessage;-;+15551234567",
            "chatIdentifier": "+15551234567",
            "participants": [{"address": "+15551234567"}]
        });
        let chat = Chat::from_bb(&raw).unwrap();
        let doc = chat.to_cache_json(8_188_273_931_022_499_394);
        assert!(doc["id"].is_string());
        assert_eq!(doc["id"], "8188273931022499394");
        assert_eq!(parse_json_id(&doc["id"]).unwrap(), 8_188_273_931_022_499_394);
    }

    #[test]
    fn contact_book_matches_plus_one_and_bare_digits() {
        let book = ContactBook::from_bb(&json!([{
            "displayName": "Pat Limp",
            "firstName": "Pat",
            "lastName": "Limp",
            "phoneNumbers": [{"address": "14035420270"}],
            "emails": []
        }]));
        assert_eq!(book.lookup("+14035420270"), Some("Pat Limp"));
        assert_eq!(book.lookup("4035420270"), Some("Pat Limp"));
        let mut chat = Chat::from_bb(&json!({
            "guid": "iMessage;-;+14035420270",
            "chatIdentifier": "+14035420270",
            "participants": [{"address": "+14035420270", "service": "iMessage"}]
        }))
        .unwrap();
        chat.apply_contacts(&book);
        assert_eq!(chat.to_cache_json(1)["contact_name"], "Pat Limp");
    }

    #[test]
    fn from_bb_chat_dm() {
        let raw = json!({
            "guid": "iMessage;-;+15551234567",
            "chatIdentifier": "+15551234567",
            "displayName": "",
            "unreadCount": 2,
            "participants": [{"address": "+15551234567", "service": "iMessage", "name": "Ada"}],
            "lastMessage": {"dateCreated": 1_700_000_000_000i64, "text": "hi"}
        });
        let chat = Chat::from_bb(&raw).unwrap();
        assert!(!chat.is_group);
        assert_eq!(chat.identifier, "+15551234567");
        assert_eq!(chat.unread_count, 2);
        let doc = chat.to_cache_json(42);
        assert_eq!(doc["id"], "42");
        assert_eq!(doc["contact_name"], "Ada");
        assert!(doc.get("chatIdentifier").is_none());
        assert!(doc.get("dateCreated").is_none());
        assert!(doc["last_message_at"].as_str().unwrap().starts_with("2023-"));
    }

    #[test]
    fn from_bb_message_uses_chats_array() {
        let raw = json!({
            "guid": "MSG-1",
            "text": "hello",
            "isFromMe": false,
            "dateCreated": 1_700_000_000_000i64,
            "handle": {"address": "+15551234567", "name": "Ada"},
            "chats": [{"guid": "iMessage;-;+15551234567"}]
        });
        let msg = Message::from_bb(&raw, None).unwrap();
        assert_eq!(msg.chat.as_str(), "iMessage;-;+15551234567");
        let doc = msg.to_cache_json(9, 42);
        assert_eq!(doc["chat_id"], "42");
        assert_eq!(doc["id"], "9");
        assert_eq!(doc["sender"], "+15551234567");
        assert_eq!(doc["sender_name"], "Ada");
        assert_eq!(doc["is_from_me"], false);
        assert!(doc.get("isFromMe").is_none());
    }

    #[test]
    fn from_bb_message_without_chat_fails() {
        let raw = json!({"guid": "MSG-1", "text": "x", "isFromMe": true, "dateCreated": 1});
        assert!(Message::from_bb(&raw, None).is_err());
        let fallback = ChatGuid::parse("iMessage;-;+1").unwrap();
        assert!(Message::from_bb(&raw, Some(&fallback)).is_ok());
    }

    #[test]
    fn unix_seconds_and_apple_epoch() {
        let unix_s = timestamp_from_bb(Some(&json!(1_700_000_000))).unwrap();
        assert!(unix_s.starts_with("2023-"));
        let apple = timestamp_from_bb(Some(&json!(694_224_000.0))).unwrap();
        assert!(apple.starts_with("2022-") || apple.starts_with("2023-"));
    }
}
