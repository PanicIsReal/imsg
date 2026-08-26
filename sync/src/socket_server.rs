use crate::cache::{ChatRow, MessageCache};
use crate::uplink::{UplinkError, UplinkHandle};
use anyhow::{Context, Result};
use imsg_proto::Envelope;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::info;

enum ClientMode {
    Oneshot,
    Streaming(broadcast::Receiver<Envelope>),
}

pub async fn serve(
    socket_path: impl AsRef<Path>,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    uplink: UplinkHandle,
) -> Result<()> {
    let socket_path = socket_path.as_ref();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).context("bind unix socket")?;
    info!("imsg-sync socket at {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let cache = Arc::clone(&cache);
        let events = events.clone();
        let uplink = uplink.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, cache, events, uplink).await {
                tracing::warn!("client error: {e}");
            }
        });
    }
}

async fn handle_client(
    stream: UnixStream,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    uplink: UplinkHandle,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut lines = BufReader::new(reader).lines();
    let mut mode = ClientMode::Oneshot;

    loop {
        match &mut mode {
            ClientMode::Streaming(rx) => {
                tokio::select! {
                    line = lines.next_line() => {
                        let Some(line) = line? else { break };
                        process_line(&line, &cache, &events, &uplink, &writer).await?;
                    }
                    evt = rx.recv() => {
                        match evt {
                            Ok(env) => write_env(&writer, &env).await?,
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                let chats = cache.read().await.list_chats(50).await?;
                                write_env(
                                    &writer,
                                    &Envelope::Event {
                                        topic: "sync.chats".into(),
                                        payload: json!({"reason": "events_lagged", "chats": chats}),
                                    },
                                )
                                .await?;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            ClientMode::Oneshot => {
                let Some(line) = lines.next_line().await? else {
                    break;
                };
                if subscribe_requested(&line)? {
                    mode = ClientMode::Streaming(events.subscribe());
                    let snap = snapshot(&cache).await?;
                    let env = Envelope::parse_line(line.trim())?;
                    if let Envelope::Req { id, .. } = env {
                        write_env(&writer, &ok_res(&id, snap)).await?;
                    }
                } else {
                    process_line(&line, &cache, &events, &uplink, &writer).await?;
                }
            }
        }
    }
    Ok(())
}

fn subscribe_requested(line: &str) -> Result<bool> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(false);
    }
    match Envelope::parse_line(line)? {
        Envelope::Req { method, .. } => Ok(method == "events.subscribe"),
        _ => Ok(false),
    }
}

async fn process_line(
    line: &str,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    uplink: &UplinkHandle,
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
) -> Result<()> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let env = Envelope::parse_line(line)?;
    if let Envelope::Req { id, method, params } = env {
        if method == "events.subscribe" {
            let snap = snapshot(cache).await?;
            write_env(writer, &ok_res(&id, snap)).await?;
            return Ok(());
        }
        let result = dispatch(cache, events, uplink, &method, params).await;
        let reply = match result {
            Ok(v) => ok_res(&id, v),
            Err(e) => {
                let code = e
                    .downcast_ref::<UplinkError>()
                    .map(UplinkError::code)
                    .unwrap_or("error");
                Envelope::Res {
                    id,
                    ok: false,
                    result: None,
                    error: Some(imsg_proto::ErrorBody {
                        code: code.into(),
                        message: e.to_string(),
                    }),
                }
            }
        };
        write_env(writer, &reply).await?;
    }
    Ok(())
}

fn ok_res(id: &str, result: Value) -> Envelope {
    Envelope::Res {
        id: id.to_string(),
        ok: true,
        result: Some(result),
        error: None,
    }
}

async fn write_env(
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    env: &Envelope,
) -> Result<()> {
    let mut w = writer.lock().await;
    w.write_all(format!("{}\n", env.to_line()?).as_bytes())
        .await?;
    Ok(())
}

async fn snapshot(cache: &Arc<RwLock<MessageCache>>) -> Result<Value> {
    let guard = cache.read().await;
    let chats = guard.list_chats(50).await?;
    let mut snap = guard.link_snapshot().await?;
    if let Some(obj) = snap.as_object_mut() {
        obj.insert("chats".into(), json!(chats));
        obj.insert("protocol".into(), json!(imsg_proto::PROTOCOL_VERSION));
    }
    Ok(snap)
}

fn live_event(applied: crate::cache::Applied) -> Envelope {
    let chat = match applied.chat {
        ChatRow::Updated(v) => Some(v),
        ChatRow::Unknown { .. } => None,
    };
    Envelope::Event {
        topic: "sync.message".into(),
        payload: json!({
            "message": applied.message,
            "chat": chat,
            "is_new": applied.is_new,
        }),
    }
}

async fn dispatch(
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    uplink: &UplinkHandle,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "status" => {
            let guard = cache.read().await;
            let mut snap = guard.link_snapshot().await?;
            if let Some(obj) = snap.as_object_mut() {
                obj.insert("connected".into(), json!(true));
                obj.insert("protocol".into(), json!(imsg_proto::PROTOCOL_VERSION));
            }
            Ok(snap)
        }
        "contacts.authorize" => Ok(uplink
            .call_timeout(
                "contacts.authorize",
                json!({}),
                std::time::Duration::from_secs(135),
            )
            .await?),
        "messages.send" => {
            let chat_id = params["chat_id"].as_i64().context("chat_id required")?;
            let text = params["text"].as_str().context("text required")?;
            let result = uplink
                .call("send", json!({"chat_id": chat_id, "text": text}))
                .await?;
            let applied = {
                let guard = cache.write().await;
                if let Some(msg) = result.get("message") {
                    Some(guard.apply_live_message(msg).await?)
                } else if result.is_object() && result.get("id").is_some() {
                    Some(guard.apply_live_message(&result).await?)
                } else {
                    None
                }
            };
            if let Some(applied) = applied {
                let _ = events.send(live_event(applied));
            }
            Ok(json!({"ok": true, "message": result}))
        }
        "chats.list" => {
            let guard = cache.read().await;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let chats = guard.list_chats(limit).await?;
            Ok(json!({"chats": chats}))
        }
        "messages.history" => {
            let guard = cache.read().await;
            let chat_id = params["chat_id"].as_i64().context("chat_id required")?;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let before = params.get("before").and_then(|v| v.as_str());
            let messages = guard.list_messages(chat_id, limit, before).await?;
            Ok(json!({"messages": messages}))
        }
        "messages.search" => {
            let guard = cache.read().await;
            let query = params["query"].as_str().context("query required")?;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let rows = sqlx::query_scalar::<_, String>(
                "SELECT raw_json FROM messages WHERE text LIKE ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(format!("%{query}%"))
            .bind(limit)
            .fetch_all(guard.pool())
            .await?;
            let messages: Vec<Value> = rows
                .iter()
                .map(|s| serde_json::from_str(s))
                .collect::<Result<_, _>>()?;
            Ok(json!({"messages": messages}))
        }
        _ => anyhow::bail!("unknown method: {method}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::MessageCache;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    async fn boot() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        broadcast::Sender<Envelope>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("imsg-sync.sock");
        let db = dir.path().join("cache.db");
        let cache = MessageCache::open(&db).await.unwrap();
        cache
            .upsert_chat(&json!({
                "id": 1,
                "name": "Ada",
                "last_message_at": "2026-01-01T00:00:00Z",
                "unread_count": 0
            }))
            .await
            .unwrap();
        let cache = Arc::new(RwLock::new(cache));
        let (tx, _) = broadcast::channel(16);
        let serve_tx = tx.clone();
        let serve_sock = sock.clone();
        tokio::spawn(async move {
            let _ = serve(serve_sock, cache, serve_tx, UplinkHandle::default()).await;
        });
        for _ in 0..100 {
            if sock.exists() {
                return (dir, sock, tx);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("unix socket was not bound");
    }

    async fn write_req(stream: &mut UnixStream, method: &str, params: Value) {
        let req = Envelope::Req {
            id: "1".into(),
            method: method.into(),
            params,
        };
        stream
            .write_all(format!("{}\n", req.to_line().unwrap()).as_bytes())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn oneshot_caller_never_receives_events() {
        let (_dir, sock, events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "chats.list", json!({"limit": 10})).await;
        let mut lines = BufReader::new(stream).lines();
        let line = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env = Envelope::parse_line(&line).unwrap();
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let chats = result.unwrap()["chats"].as_array().unwrap().clone();
                assert_eq!(chats.len(), 1);
            }
            other => panic!("expected chats.list res, got {other:?}"),
        }

        let _ = events.send(Envelope::Event {
            topic: "sync.message".into(),
            payload: json!({"is_new": true}),
        });
        let extra = timeout(Duration::from_millis(150), lines.next_line()).await;
        assert!(
            extra.is_err(),
            "oneshot connection must not be written an event"
        );
    }

    #[tokio::test]
    async fn subscribe_receives_snapshot_then_events() {
        let (_dir, sock, events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "events.subscribe", json!({})).await;
        let mut lines = BufReader::new(stream).lines();
        let snap = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env = Envelope::parse_line(&snap).unwrap();
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let result = result.unwrap();
                assert_eq!(result["chats"].as_array().unwrap().len(), 1);
            }
            other => panic!("expected subscribe snapshot, got {other:?}"),
        }

        let _ = events.send(Envelope::Event {
            topic: "sync.message".into(),
            payload: json!({"message": {"id": 9, "chat_id": 1}, "is_new": true}),
        });
        let pushed = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env = Envelope::parse_line(&pushed).unwrap();
        match env {
            Envelope::Event { topic, payload } => {
                assert_eq!(topic, "sync.message");
                assert_eq!(payload["message"]["id"], 9);
            }
            other => panic!("expected sync.message event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_and_snapshot_include_contacts_from_meta() {
        let (_dir, sock, _events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "status", json!({})).await;
        let mut lines = BufReader::new(stream).lines();
        let line = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env = Envelope::parse_line(&line).unwrap();
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                assert_eq!(result.unwrap()["contacts"], "unknown");
            }
            other => panic!("expected status res, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn contacts_authorize_without_uplink_is_link_down() {
        let (_dir, sock, _events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "contacts.authorize", json!({})).await;
        let mut lines = BufReader::new(stream).lines();
        let line = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env = Envelope::parse_line(&line).unwrap();
        match env {
            Envelope::Res {
                ok: false, error, ..
            } => {
                assert_eq!(error.unwrap().code, "link_down");
            }
            other => panic!("expected link_down, got {other:?}"),
        }
    }
}
