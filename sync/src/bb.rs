use crate::domain::{
    attachment_form_guid, AttachmentMeta, Chat, ChatGuid, ContactBook, Message, MessageGuid,
};
use crate::link::Credentials;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum BbError {
    #[error("mac link is down")]
    LinkDown,
    #[error("timed out awaiting {0}")]
    Timeout(String),
    #[error("{0}")]
    Upstream(String),
    #[error(transparent)]
    Transport(#[from] anyhow::Error),
}

impl BbError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LinkDown => "link_down",
            Self::Timeout(_) => "timeout",
            Self::Upstream(_) => "upstream",
            Self::Transport(_) => "error",
        }
    }
}

pub struct BlueBubbles {
    http: reqwest::Client,
    creds: Credentials,
    contacts: RwLock<ContactBook>,
}

pub struct Subscription {
    pub events: mpsc::Receiver<Message>,
    pub pump: JoinHandle<Result<()>>,
}

impl BlueBubbles {
    pub fn new(creds: Credentials) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("http client")?;
        Ok(Self {
            http,
            creds,
            contacts: RwLock::new(ContactBook::default()),
        })
    }

    pub async fn connect(creds: Credentials) -> Result<Arc<Self>> {
        let client = Arc::new(Self::new(creds)?);
        client.ping().await?;
        Ok(client)
    }

    pub async fn ping(&self) -> Result<(), BbError> {
        let body = self.get("api/v1/ping").await?;
        let data = envelope_data(&body)?;
        if data.as_str() == Some("pong")
            || data["message"].as_str() == Some("pong")
            || body["status"].as_i64() == Some(200)
        {
            Ok(())
        } else {
            Err(BbError::Upstream("ping failed".into()))
        }
    }

    pub async fn query_chats(&self, limit: u32) -> Result<Vec<Chat>, BbError> {
        let body = self
            .post(
                "api/v1/chat/query",
                json!({"limit": limit, "offset": 0, "with": ["lastMessage", "participants"]}),
            )
            .await?;
        let data = envelope_data(&body)?;
        let arr = data.as_array().cloned().unwrap_or_default();
        arr.iter()
            .map(Chat::from_bb)
            .collect::<Result<Vec<_>>>()
            .map_err(|e| BbError::Upstream(e.to_string()))
    }

    pub async fn query_contacts(&self) -> Result<ContactBook, BbError> {
        let body = self.get("api/v1/contact").await?;
        let data = envelope_data(&body)?;
        let book = ContactBook::from_bb(&data);
        *self.contacts.write().await = book.clone();
        Ok(book)
    }

    pub async fn contact_book(&self) -> ContactBook {
        self.contacts.read().await.clone()
    }

    pub async fn chat_messages(&self, chat: &ChatGuid, limit: u32) -> Result<Vec<Message>, BbError> {
        let encoded = path_encode(chat.as_str());
        let body = self
            .get(&format!(
                "api/v1/chat/{encoded}/message?limit={limit}&offset=0&sort=DESC"
            ))
            .await?;
        let data = envelope_data(&body)?;
        let arr = data.as_array().cloned().unwrap_or_default();
        arr.iter()
            .map(|v| Message::from_bb(v, Some(chat)))
            .collect::<Result<Vec<_>>>()
            .map_err(|e| BbError::Upstream(e.to_string()))
    }

    pub async fn send_text(&self, chat: &ChatGuid, text: &str) -> Result<Message, BbError> {
        let temp = format!("temp-{}", Uuid::new_v4());
        let body = self
            .post(
                "api/v1/message/text",
                json!({
                    "chatGuid": chat.as_str(),
                    "tempGuid": temp,
                    "message": text,
                }),
            )
            .await?;
        let data = envelope_data(&body)?;
        message_from_send_response(data, chat, text, &temp, None)
    }

    pub async fn send_attachment(
        &self,
        chat: &ChatGuid,
        identifier: &str,
        path: &Path,
    ) -> Result<Message, BbError> {
        let path = readable_image_path(path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("photo.jpg")
            .to_string();
        let bytes = std::fs::read(&path).map_err(|e| BbError::Upstream(e.to_string()))?;
        let temp = format!("temp-{}", Uuid::new_v4());
        let form_guid = attachment_form_guid(chat, identifier);
        let mut url = self.authed("api/v1/message/attachment")?;
        url.query_pairs_mut().append_pair("chatGuid", chat.as_str());
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| BbError::Upstream(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("chatGuid", form_guid)
            .text("tempGuid", temp.clone())
            .text("name", name.clone())
            .part("attachment", part);
        let res = self
            .http
            .post(url)
            .timeout(Duration::from_secs(120))
            .multipart(form)
            .send()
            .await
            .map_err(transport)?;
        let body = read_json(res).await?;
        let data = envelope_data(&body)?;
        let mut msg = message_from_send_response(data, chat, "", &temp, Some(&name))?;
        if msg.attachments.is_empty() {
            msg.attachments.push(AttachmentMeta {
                guid: temp,
                mime: None,
                name: Some(name),
            });
        }
        Ok(msg)
    }

    pub async fn recent_messages(&self, limit: u32) -> Result<Vec<Message>, BbError> {
        let body = self
            .post(
                "api/v1/message/query",
                json!({"limit": limit, "offset": 0, "with": ["chats"]}),
            )
            .await?;
        let data = envelope_data(&body)?;
        let arr = data.as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        for v in arr {
            match Message::from_bb(&v, None) {
                Ok(m) => out.push(m),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    pub fn subscribe(self: Arc<Self>) -> Subscription {
        let (tx, rx) = mpsc::channel(256);
        let pump = tokio::spawn(async move { poll_loop(self, tx).await });
        Subscription { events: rx, pump }
    }

    async fn get(&self, path: &str) -> Result<Value, BbError> {
        let url = self.authed(path)?;
        let res = self.http.get(url).send().await.map_err(transport)?;
        read_json(res).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, BbError> {
        let url = self.authed(path)?;
        let res = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        read_json(res).await
    }

    fn authed(&self, path: &str) -> Result<reqwest::Url, BbError> {
        let mut url = self
            .creds
            .public
            .server
            .join(path)
            .map_err(BbError::Transport)?;
        url.query_pairs_mut()
            .append_pair("password", self.creds.password().as_str());
        Ok(url)
    }
}

fn transport(e: reqwest::Error) -> BbError {
    if e.is_timeout() {
        BbError::Timeout("bluebubbles".into())
    } else if e.is_connect() {
        BbError::LinkDown
    } else {
        BbError::Transport(e.into())
    }
}

async fn read_json(res: reqwest::Response) -> Result<Value, BbError> {
    let status = res.status();
    let body = res.text().await.map_err(transport)?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(BbError::Upstream("unauthorized".into()));
    }
    if !status.is_success() {
        return Err(BbError::Upstream(format!("http {status}: {body}")));
    }
    serde_json::from_str(&body).map_err(|e| BbError::Upstream(e.to_string()))
}

fn path_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn readable_image_path(path: &Path) -> Result<PathBuf, BbError> {
    let path = path.canonicalize().map_err(|e| BbError::Upstream(e.to_string()))?;
    if !path.is_file() {
        return Err(BbError::Upstream("not a file".into()));
    }
    let meta = path.metadata().map_err(|e| BbError::Upstream(e.to_string()))?;
    if meta.len() > 25 * 1024 * 1024 {
        return Err(BbError::Upstream("image larger than 25 MB".into()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "bmp" => Ok(path),
        _ => Err(BbError::Upstream("not an image".into())),
    }
}

fn message_from_send_response(
    data: Value,
    chat: &ChatGuid,
    text: &str,
    temp: &str,
    attachment: Option<&str>,
) -> Result<Message, BbError> {
    let payload = if data
        .get("guid")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        data
    } else if let Some(inner) = data.get("message").cloned() {
        inner
    } else if let Some(first) = data.as_array().and_then(|a| a.first()).cloned() {
        first
    } else {
        data
    };
    if let Ok(msg) = Message::from_bb(&payload, Some(chat)) {
        return Ok(msg);
    }
    let attachments = attachment
        .map(|name| {
            vec![AttachmentMeta {
                guid: temp.to_string(),
                mime: None,
                name: Some(name.to_string()),
            }]
        })
        .unwrap_or_default();
    Ok(Message {
        guid: MessageGuid::parse(temp).map_err(|e| BbError::Upstream(e.to_string()))?,
        chat: chat.clone(),
        text: text.to_string(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        is_from_me: true,
        sender: None,
        attachments,
    })
}

fn envelope_data(body: &Value) -> Result<Value, BbError> {
    if let Some(err) = body.get("error") {
        if !err.is_null() {
            let msg = err["error"]
                .as_str()
                .or(err.as_str())
                .or(body["message"].as_str())
                .unwrap_or("upstream error");
            return Err(BbError::Upstream(msg.into()));
        }
    }
    if let Some(status) = body["status"].as_i64() {
        if status != 200 {
            return Err(BbError::Upstream(
                body["message"].as_str().unwrap_or("upstream error").into(),
            ));
        }
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

async fn poll_loop(client: Arc<BlueBubbles>, tx: mpsc::Sender<Message>) -> Result<()> {
    let mut seen: VecDeque<String> = VecDeque::new();
    loop {
        match client.recent_messages(40).await {
            Ok(msgs) => {
                for msg in msgs.into_iter().rev() {
                    let g = msg.guid.as_str().to_string();
                    if seen.iter().any(|s| s == &g) {
                        continue;
                    }
                    seen.push_back(g);
                    while seen.len() > 2000 {
                        seen.pop_front();
                    }
                    if tx.send(msg).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Err(BbError::LinkDown) => return Err(anyhow::anyhow!("link down")),
            Err(e) => tracing::warn!("poll: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub fn dedupe_guid(seen: &mut VecDeque<MessageGuid>, guid: &MessageGuid) -> bool {
    if seen.iter().any(|g| g == guid) {
        return false;
    }
    seen.push_back(guid.clone());
    while seen.len() > 2000 {
        seen.pop_front();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_ok_and_error() {
        let ok = json!({"status": 200, "message": "ok", "data": [{"guid": "x"}]});
        assert!(envelope_data(&ok).unwrap().as_array().is_some());
        let err = json!({"status": 401, "message": "no", "error": {"error": "bad password"}});
        assert!(envelope_data(&err).is_err());
    }

    #[test]
    fn dedupe_first_then_repeat() {
        let mut seen = VecDeque::new();
        let g = MessageGuid::parse("A").unwrap();
        assert!(dedupe_guid(&mut seen, &g));
        assert!(!dedupe_guid(&mut seen, &g));
    }

    #[test]
    fn send_response_uses_message_body_when_guid_present() {
        let chat = ChatGuid::parse("iMessage;-;+15551212").unwrap();
        let data = json!({
            "guid": "MSG-1",
            "text": "hi",
            "isFromMe": true,
            "dateCreated": 1_700_000_000_000i64,
        });
        let msg = message_from_send_response(data, &chat, "hi", "temp-x", None).unwrap();
        assert_eq!(msg.guid.as_str(), "MSG-1");
        assert_eq!(msg.text, "hi");
        assert!(msg.is_from_me);
        assert_eq!(msg.chat.as_str(), "iMessage;-;+15551212");
    }

    #[test]
    fn send_response_unwraps_nested_message_and_stubs_missing_guid() {
        let chat = ChatGuid::parse("iMessage;-;+15551212").unwrap();
        let nested = json!({"message": {"text": "hi", "isFromMe": true}});
        let msg = message_from_send_response(nested, &chat, "hi", "temp-nested", None).unwrap();
        assert_eq!(msg.guid.as_str(), "temp-nested");
        assert_eq!(msg.text, "hi");
        assert!(msg.is_from_me);

        let empty = json!({"status": "queued"});
        let stub = message_from_send_response(empty, &chat, "queued", "temp-empty", None).unwrap();
        assert_eq!(stub.guid.as_str(), "temp-empty");
        assert_eq!(stub.text, "queued");
    }

    #[test]
    fn send_envelope_error_is_not_a_successful_stub() {
        let err = json!({"status": 400, "message": "not delivered", "error": {"error": "not delivered"}});
        assert!(envelope_data(&err).is_err());
    }

    #[test]
    fn readable_image_path_rejects_missing_and_non_image() {
        let dir = tempfile::tempdir().unwrap();
        assert!(readable_image_path(&dir.path().join("nope.png")).is_err());
        let txt = dir.path().join("note.txt");
        std::fs::write(&txt, b"hi").unwrap();
        assert!(readable_image_path(&txt).is_err());
        let png = dir.path().join("pic.png");
        std::fs::write(&png, b"not-really-png").unwrap();
        assert_eq!(readable_image_path(&png).unwrap(), png.canonicalize().unwrap());
    }
}
