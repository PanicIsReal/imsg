use crate::cache::{ChatRow, MessageCache};
use crate::uplink::{UplinkError, UplinkHandle};
use anyhow::{Context, Result};
use imsg_proto::Envelope;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, RwLock, Semaphore};
use tracing::info;

enum ClientMode {
    Oneshot,
    Streaming(broadcast::Receiver<Envelope>),
}

const CLIENT_OUTPUT_CAPACITY: usize = 64;
const CLIENT_REQUEST_LIMIT: usize = 16;

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
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
    let (output_tx, mut output_rx) = mpsc::channel::<Envelope>(CLIENT_OUTPUT_CAPACITY);
    let mut writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(env) = output_rx.recv().await {
            writer
                .write_all(format!("{}\n", env.to_line()?).as_bytes())
                .await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let _writer_guard = AbortOnDrop(writer_task.abort_handle());
    let request_slots = Arc::new(Semaphore::new(CLIENT_REQUEST_LIMIT));
    let mut requests = tokio::task::JoinSet::new();
    let mut lines = BufReader::new(reader).lines();
    let mut mode = ClientMode::Oneshot;
    let mut resync_needed = false;

    loop {
        match &mut mode {
            ClientMode::Streaming(rx) => {
                tokio::select! {
                    line = lines.next_line() => {
                        let Some(line) = line? else { break };
                        spawn_request(&line, &cache, &events, &uplink, &output_tx, &request_slots, &mut requests)?;
                    }
                    evt = rx.recv() => {
                        match evt {
                            Ok(env) => {
                                if output_tx.try_send(env).is_err() {
                                    resync_needed = true;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                resync_needed = true;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    Some(result) = requests.join_next(), if !requests.is_empty() => result??,
                    permit = output_tx.reserve(), if resync_needed => {
                        permit?.send(local_resync(&cache, "events_lagged").await?);
                        resync_needed = false;
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
                        output_tx.send(ok_res(&id, snap)).await?;
                    }
                } else {
                    spawn_request(
                        &line,
                        &cache,
                        &events,
                        &uplink,
                        &output_tx,
                        &request_slots,
                        &mut requests,
                    )?;
                    if let Some(result) = requests.join_next().await {
                        result??;
                    }
                }
            }
        }
    }
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    drop(output_tx);
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
        let _ = writer_task.await;
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

fn spawn_request(
    line: &str,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    uplink: &UplinkHandle,
    output: &mpsc::Sender<Envelope>,
    slots: &Arc<Semaphore>,
    requests: &mut tokio::task::JoinSet<Result<()>>,
) -> Result<()> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let env = Envelope::parse_line(line)?;
    if let Envelope::Req { id, method, params } = env {
        let permit = match Arc::clone(slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = output.try_send(Envelope::Res {
                    id,
                    ok: false,
                    result: None,
                    error: Some(imsg_proto::ErrorBody {
                        code: "busy".into(),
                        message: "too many in-flight client requests".into(),
                    }),
                });
                return Ok(());
            }
        };
        let cache = Arc::clone(cache);
        let events = events.clone();
        let uplink = uplink.clone();
        let output = output.clone();
        requests.spawn(async move {
            let _permit = permit;
            let result = if method == "events.subscribe" {
                snapshot(&cache).await
            } else {
                dispatch(&cache, &events, &uplink, &id, &method, params).await
            };
            let reply = match result {
                Ok(v) => ok_res(&id, v),
                Err(e) => error_res(id, e),
            };
            output.send(reply).await.context("client output closed")
        });
    }
    Ok(())
}

fn error_res(id: String, error: anyhow::Error) -> Envelope {
    let code = error
        .downcast_ref::<UplinkError>()
        .map(UplinkError::code)
        .unwrap_or("error");
    Envelope::Res {
        id,
        ok: false,
        result: None,
        error: Some(imsg_proto::ErrorBody {
            code: code.into(),
            message: error.to_string(),
        }),
    }
}

fn ok_res(id: &str, result: Value) -> Envelope {
    Envelope::Res {
        id: id.to_string(),
        ok: true,
        result: Some(result),
        error: None,
    }
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

async fn local_resync(cache: &Arc<RwLock<MessageCache>>, reason: &str) -> Result<Envelope> {
    let guard = cache.read().await;
    let chats = guard.list_chats(50).await?;
    let generation = guard.get_meta("db_generation").await?.unwrap_or_default();
    Ok(Envelope::Event {
        topic: "sync.resync".into(),
        payload: json!({"reason": reason, "chats": chats, "db_generation": generation}),
    })
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
    request_id: &str,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        #[cfg(test)]
        "test.sleep" => {
            let millis = params.get("millis").and_then(Value::as_u64).unwrap_or(0);
            tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
            Ok(json!({"slept_ms": millis}))
        }
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
            let generation = cache.read().await.get_meta("db_generation").await?;
            let client_id = params
                .get("client_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let started = Instant::now();
            let result = uplink
                .call("send", json!({"chat_id": chat_id, "text": text}))
                .await?;
            tracing::debug!(
                metric = "send_roundtrip",
                elapsed_ms = started.elapsed().as_millis() as u64,
                request_id = %request_id,
                "realtime latency"
            );
            let applied = {
                let mut msg = if let Some(inner) = result.get("message") {
                    inner.clone()
                } else if result.is_object() && result.get("id").is_some() {
                    result.clone()
                } else {
                    Value::Null
                };
                if msg.is_object() {
                    if let Some(client_id) = &client_id {
                        msg["client_id"] = json!(client_id);
                    }
                    if msg.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0) == 0 {
                        msg["chat_id"] = json!(chat_id);
                    }
                    if msg.get("is_from_me").is_none() {
                        msg["is_from_me"] = json!(true);
                    }
                    if msg.get("text").and_then(|v| v.as_str()).is_none() {
                        msg["text"] = json!(text);
                    }
                    let guard = cache.write().await;
                    if guard.get_meta("db_generation").await? != generation {
                        anyhow::bail!("send outcome unconfirmed after database generation changed");
                    }
                    Some(guard.apply_live_message(&msg).await?)
                } else {
                    None
                }
            };
            if let Some(applied) = applied {
                let _ = events.send(live_event(applied));
            }
            Ok(json!({"ok": true, "message": result, "client_id": client_id}))
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
        write_req_with_id(stream, "1", method, params).await;
    }

    async fn write_req_with_id(stream: &mut UnixStream, id: &str, method: &str, params: Value) {
        let req = Envelope::Req {
            id: id.into(),
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

    #[tokio::test]
    async fn half_closed_oneshot_still_receives_its_response() {
        let (_dir, sock, _events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "status", json!({})).await;
        stream.shutdown().await.unwrap();
        let line = timeout(
            Duration::from_secs(1),
            BufReader::new(stream).lines().next_line(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(matches!(
            Envelope::parse_line(&line).unwrap(),
            Envelope::Res { ok: true, .. }
        ));
    }

    #[tokio::test]
    async fn slow_streaming_request_does_not_block_events_or_later_requests() {
        let (_dir, sock, events) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "events.subscribe", json!({})).await;
        let mut lines = BufReader::new(stream).lines();
        lines.next_line().await.unwrap().unwrap();

        let stream = lines.get_mut().get_mut();
        write_req_with_id(stream, "slow", "test.sleep", json!({"millis": 300})).await;
        write_req_with_id(stream, "fast", "status", json!({})).await;
        let _ = events.send(Envelope::Event {
            topic: "sync.message".into(),
            payload: json!({"message": {"id": 7}}),
        });

        for _ in 0..2 {
            let line = timeout(Duration::from_millis(150), lines.next_line())
                .await
                .expect("event and fast response must beat the slow request")
                .unwrap()
                .unwrap();
            match Envelope::parse_line(&line).unwrap() {
                Envelope::Res { id, .. } => assert_ne!(id, "slow"),
                Envelope::Event { topic, .. } => assert_eq!(topic, "sync.message"),
                other => panic!("unexpected envelope {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn lag_recovery_event_names_reason_and_includes_chats() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        cache
            .upsert_chat(&json!({"id": 1, "last_message_at": "2026-01-01T00:00:00Z"}))
            .await
            .unwrap();
        let env = local_resync(&Arc::new(RwLock::new(cache)), "events_lagged")
            .await
            .unwrap();
        let Envelope::Event { topic, payload } = env else {
            panic!("expected event");
        };
        assert_eq!(topic, "sync.resync");
        assert_eq!(payload["reason"], "events_lagged");
        assert_eq!(payload["chats"].as_array().unwrap().len(), 1);
        assert_eq!(payload["db_generation"], "");
    }
}
