use crate::domain::{Chat, ChatGuid, Message, MessageGuid};
use crate::link::Credentials;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
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
        Ok(Self { http, creds })
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
        let body = self
            .post(
                "api/v1/message/text",
                json!({
                    "chatGuid": chat.as_str(),
                    "tempGuid": format!("temp-{}", Uuid::new_v4()),
                    "message": text,
                }),
            )
            .await?;
        let data = envelope_data(&body)?;
        Message::from_bb(&data, Some(chat)).map_err(|e| BbError::Upstream(e.to_string()))
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

#[allow(dead_code)]
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
}
