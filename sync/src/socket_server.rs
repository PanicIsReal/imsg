use crate::cache::{ChatRow, MessageCache};
use crate::domain::ChatGuid;
use crate::link::{emit_sync_link, merge_view, Link, SettingsDraft, WebhookDraft};
use crate::uplink::UplinkError;
use anyhow::{Context, Result};
use imsg_proto::Envelope;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::info;

enum ClientMode {
    Oneshot,
    Streaming(broadcast::Receiver<Envelope>),
}

const MAX_IN_FLIGHT_REQUESTS: usize = 8;

pub async fn serve(
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    link: Arc<Link>,
) -> Result<()> {
    let socket_path = link.socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).context("bind unix socket")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    info!("imsg-sync socket at {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let cache = Arc::clone(&cache);
        let events = events.clone();
        let link = Arc::clone(&link);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, cache, events, link).await {
                tracing::warn!("client error: {e}");
            }
        });
    }
}

async fn handle_client(
    stream: UnixStream,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    link: Arc<Link>,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut lines = BufReader::new(reader).lines();
    let mut mode = ClientMode::Oneshot;
    let permits = Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS));
    let mut requests = JoinSet::new();

    loop {
        while requests.try_join_next().is_some() {}
        match &mut mode {
            ClientMode::Streaming(rx) => {
                tokio::select! {
                    biased;
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
                    _ = requests.join_next(), if !requests.is_empty() => {}
                    line = lines.next_line() => {
                        let Some(line) = line? else { break };
                        start_request(&line, &cache, &events, &link, &writer, &permits, &mut requests).await?;
                    }
                }
            }
            ClientMode::Oneshot => {
                let Some(line) = lines.next_line().await? else {
                    break;
                };
                if subscribe_requested(&line)? {
                    mode = ClientMode::Streaming(events.subscribe());
                    let snap = snapshot(&cache, &link).await?;
                    let env = Envelope::parse_line(line.trim())?;
                    if let Envelope::Req { id, .. } = env {
                        write_env(&writer, &ok_res(&id, snap)).await?;
                    }
                } else {
                    start_request(
                        &line,
                        &cache,
                        &events,
                        &link,
                        &writer,
                        &permits,
                        &mut requests,
                    )
                    .await?;
                }
            }
        }
    }
    requests.abort_all();
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

async fn start_request(
    line: &str,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    link: &Arc<Link>,
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    permits: &Arc<Semaphore>,
    requests: &mut JoinSet<()>,
) -> Result<()> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let env = Envelope::parse_line(line)?;
    if let Envelope::Req { id, method, params } = env {
        if method == "events.subscribe" {
            let snap = snapshot(cache, link).await?;
            write_env(writer, &ok_res(&id, snap)).await?;
            return Ok(());
        }
        let permit = match Arc::clone(permits).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let reply = error_res(&id, "busy", "too many requests in flight");
                write_env(writer, &reply).await?;
                return Ok(());
            }
        };
        let cache = Arc::clone(cache);
        let events = events.clone();
        let link = Arc::clone(link);
        let writer = Arc::clone(writer);
        requests.spawn(async move {
            let _permit = permit;
            let result = dispatch(&cache, &events, &link, &method, params).await;
            let reply = match result {
                Ok(v) => ok_res(&id, v),
                Err(e) => {
                    let code = e
                        .downcast_ref::<UplinkError>()
                        .map(UplinkError::code)
                        .unwrap_or("error");
                    error_res(&id, code, &e.to_string())
                }
            };
            if let Err(e) = write_env(&writer, &reply).await {
                tracing::debug!("request reply failed: {e}");
            }
        });
    }
    Ok(())
}

fn error_res(id: &str, code: &str, message: &str) -> Envelope {
    Envelope::Res {
        id: id.to_string(),
        ok: false,
        result: None,
        error: Some(imsg_proto::ErrorBody {
            code: code.into(),
            message: message.into(),
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

async fn write_env(
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    env: &Envelope,
) -> Result<()> {
    let mut w = writer.lock().await;
    w.write_all(format!("{}\n", env.to_line()?).as_bytes())
        .await?;
    Ok(())
}

async fn snapshot(cache: &Arc<RwLock<MessageCache>>, link: &Arc<Link>) -> Result<Value> {
    let guard = cache.read().await;
    let chats = guard.list_chats(50).await?;
    let mut snap = guard.link_snapshot().await?;
    drop(guard);
    merge_view(&mut snap, &link.view().await);
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
    link: &Arc<Link>,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "status" => {
            let guard = cache.read().await;
            let mut snap = guard.link_snapshot().await?;
            drop(guard);
            if let Some(obj) = snap.as_object_mut() {
                obj.insert("connected".into(), json!(true));
                obj.insert("protocol".into(), json!(imsg_proto::PROTOCOL_VERSION));
            }
            merge_view(&mut snap, &link.view().await);
            Ok(snap)
        }
        "config.set" => {
            let url = params["server_url"]
                .as_str()
                .context("server_url required")?;
            let password = params.get("password").and_then(|v| v.as_str());
            let draft = SettingsDraft::from_input(url, password)?;
            let view = link.apply(draft).await?;
            emit_sync_link(events, cache, &view).await;
            Ok(view.to_status_fields())
        }
        "config.reconnect" => {
            let view = link.reconnect().await?;
            emit_sync_link(events, cache, &view).await;
            Ok(view.to_status_fields())
        }
        "webhook.set" => {
            let enabled = params["enabled"].as_bool().unwrap_or(false);
            let port = params
                .get("port")
                .and_then(|p| p.as_u64())
                .unwrap_or(crate::webhook::DEFAULT_PORT as u64) as u16;
            let serve_url = params
                .get("serve_url")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            if !enabled {
                let _ = link.uplink().webhook_clear_ours().await;
            }
            let view = link
                .apply_webhook(WebhookDraft {
                    enabled,
                    port,
                    serve_url,
                })
                .await?;
            emit_sync_link(events, cache, &view).await;
            Ok(view.to_status_fields())
        }
        "webhook.url" => {
            let url = link.webhook_copy_url()?;
            Ok(json!({ "url": url }))
        }
        "webhook.rotate" => {
            let _ = link.uplink().webhook_clear_ours().await;
            let view = link.rotate_webhook_token().await?;
            emit_sync_link(events, cache, &view).await;
            Ok(view.to_status_fields())
        }
        "webhook.register" => {
            if !link.uplink().is_up().await {
                anyhow::bail!("Connect to BlueBubbles first");
            }
            let url = link.webhook_copy_url()?;
            link.uplink()
                .webhook_replace(&url)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            link.set_webhook_registered(true);
            let view = link.view().await;
            emit_sync_link(events, cache, &view).await;
            Ok(view.to_status_fields())
        }
        "contacts.authorize" => {
            let book = link.uplink().contact_book().await?;
            let n = cache.write().await.apply_contact_book(&book).await?;
            let chats = cache.read().await.list_chats(50).await?;
            let _ = events.send(Envelope::Event {
                topic: "sync.chats".into(),
                payload: json!({"reason": "contacts", "chats": chats}),
            });
            let names_visible = n > 0
                || chats.iter().any(|c| {
                    c["contact_name"]
                        .as_str()
                        .is_some_and(|s| s.chars().any(|ch| ch.is_alphabetic()))
                });
            Ok(json!({
                "outcome": if names_visible { "granted" } else { "unavailable" },
                "names_visible": names_visible
            }))
        }
        "chats.mark_read" => {
            let chat_id =
                crate::domain::parse_json_id(&params["chat_id"]).context("chat_id required")?;
            let chat = cache.write().await.mark_read(chat_id).await?;
            if let Some(guid) = cache.read().await.guid_for_chat_id(chat_id).await? {
                if let Ok(guid) = ChatGuid::parse(guid) {
                    let _ = link.uplink().mark_read(&guid).await;
                }
            }
            Ok(json!({"chat": chat}))
        }
        "messages.send" => {
            let chat_id =
                crate::domain::parse_json_id(&params["chat_id"]).context("chat_id required")?;
            let text = params["text"].as_str().context("text required")?;
            let guid = cache
                .read()
                .await
                .guid_for_chat_id(chat_id)
                .await?
                .context("unknown chat")?;
            let guid = ChatGuid::parse(guid)?;
            let msg = link.uplink().send_text(&guid, text).await?;
            let mut applied = cache.write().await.apply_domain_message(&msg).await?;
            echo_client_id(&mut applied.message, &params);
            let out = applied.message.clone();
            let _ = events.send(live_event(applied));
            Ok(json!({"ok": true, "message": out}))
        }
        "messages.send_attachment" => {
            let chat_id =
                crate::domain::parse_json_id(&params["chat_id"]).context("chat_id required")?;
            let path = params["path"].as_str().context("path required")?;
            let guard = cache.read().await;
            let guid = guard
                .guid_for_chat_id(chat_id)
                .await?
                .context("unknown chat")?;
            let identifier = guard
                .identifier_for_chat_id(chat_id)
                .await?
                .unwrap_or_default();
            drop(guard);
            let guid = ChatGuid::parse(guid)?;
            let msg = link
                .uplink()
                .send_attachment(&guid, &identifier, std::path::Path::new(path))
                .await?;
            let applied = cache.write().await.apply_domain_message(&msg).await?;
            let out = applied.message.clone();
            let _ = events.send(live_event(applied));
            Ok(json!({"ok": true, "message": out}))
        }
        "chats.list" => {
            let guard = cache.read().await;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(80);
            let chats = guard.list_chats(limit).await?;
            Ok(json!({"chats": chats}))
        }
        "messages.history" => {
            let guard = cache.read().await;
            let chat_id =
                crate::domain::parse_json_id(&params["chat_id"]).context("chat_id required")?;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(200);
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

fn echo_client_id(message: &mut Value, params: &Value) {
    let Some(client_id) = params.get("client_id").and_then(Value::as_str) else {
        return;
    };
    if let Some(message) = message.as_object_mut() {
        message.insert("client_id".into(), json!(client_id));
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

    #[test]
    fn send_message_echoes_client_id_into_local_payload() {
        let mut message = json!({"id": "server-guid", "text": "hello"});
        echo_client_id(&mut message, &json!({"client_id": "local-7"}));
        assert_eq!(message["client_id"], "local-7");
    }

    async fn boot() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        broadcast::Sender<Envelope>,
        Arc<Link>,
        Arc<RwLock<MessageCache>>,
    ) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let link = Link::boot_isolated(dir.path()).unwrap();
        let sock = link.socket_path().to_path_buf();
        let cache = MessageCache::open(link.cache_path()).await.unwrap();
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
        let serve_link = Arc::clone(&link);
        let serve_cache = Arc::clone(&cache);
        tokio::spawn(async move {
            let _ = serve(serve_cache, serve_tx, serve_link).await;
        });
        for _ in 0..100 {
            if sock.exists() {
                return (dir, sock, tx, link, cache);
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

    async fn read_res(stream: UnixStream) -> Envelope {
        let mut lines = BufReader::new(stream).lines();
        let line = timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        Envelope::parse_line(&line).unwrap()
    }

    fn assert_no_password_key(value: &Value) {
        let obj = value.as_object().expect("object result");
        assert!(
            !obj.keys().any(|k| {
                let n = k.to_ascii_lowercase();
                n == "password" || n == "token" || n == "webhook_token"
            }),
            "result leaked a secret key: {value}"
        );
        let blob = value.to_string();
        assert!(
            !blob.contains("token="),
            "status leaked a token query: {value}"
        );
    }

    #[tokio::test]
    async fn oneshot_caller_never_receives_events() {
        let (_dir, sock, events, _link, _cache) = boot().await;
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
        let (_dir, sock, events, _link, _cache) = boot().await;
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
    async fn subscribed_events_pass_a_blocked_request() {
        let (_dir, sock, events, _link, cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "events.subscribe", json!({})).await;
        let mut lines = BufReader::new(stream).lines();
        timeout(Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let guard = cache.write().await;
        let request = Envelope::Req {
            id: "blocked".into(),
            method: "chats.list".into(),
            params: json!({"limit": 10}),
        };
        lines
            .get_mut()
            .write_all(format!("{}\n", request.to_line().unwrap()).as_bytes())
            .await
            .unwrap();
        tokio::task::yield_now().await;
        let _ = events.send(Envelope::Event {
            topic: "sync.message".into(),
            payload: json!({"message": {"id": 10}, "is_new": true}),
        });

        let line = timeout(Duration::from_millis(250), lines.next_line())
            .await
            .expect("event was delayed behind a blocked request")
            .unwrap()
            .unwrap();
        drop(guard);
        match Envelope::parse_line(&line).unwrap() {
            Envelope::Event { topic, payload } => {
                assert_eq!(topic, "sync.message");
                assert_eq!(payload["message"]["id"], 10);
            }
            other => panic!("expected event before response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_webhook_flags_have_no_token() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "status", json!({})).await;
        let env = read_res(stream).await;
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let result = result.unwrap();
                assert_eq!(result["webhook_enabled"], false);
                assert_eq!(result["webhook_port"], crate::webhook::DEFAULT_PORT);
                assert_no_password_key(&result);
            }
            other => panic!("expected status res, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn webhook_url_includes_token_query() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "webhook.url", json!({})).await;
        let env = read_res(stream).await;
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let result = result.unwrap();
                let url = result["url"].as_str().unwrap();
                assert!(url.contains("/imsg/hook?"));
                assert!(url.contains("token="));
            }
            other => panic!("expected webhook.url res, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_and_snapshot_include_contacts_from_meta() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "status", json!({})).await;
        let env = read_res(stream).await;
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let result = result.unwrap();
                assert_eq!(result["contacts"], "unknown");
                assert_eq!(result["session"], "unconfigured");
                assert_eq!(result["password_set"], false);
                assert_no_password_key(&result);
            }
            other => panic!("expected status res, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn contacts_authorize_without_uplink_is_link_down() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "contacts.authorize", json!({})).await;
        let env = read_res(stream).await;
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
    async fn config_set_then_status_has_url_and_password_set_without_secret() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(
            &mut stream,
            "config.set",
            json!({
                "server_url": "100.64.1.2",
                "password": "s3cret"
            }),
        )
        .await;
        let env = read_res(stream).await;
        let result = match env {
            Envelope::Res {
                ok: true, result, ..
            } => result.unwrap(),
            other => panic!("expected config.set res, got {other:?}"),
        };
        assert_eq!(result["server_url"], "http://100.64.1.2:1234");
        assert_eq!(result["password_set"], true);
        assert_no_password_key(&result);

        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "status", json!({})).await;
        let env = read_res(stream).await;
        let result = match env {
            Envelope::Res {
                ok: true, result, ..
            } => result.unwrap(),
            other => panic!("expected status res, got {other:?}"),
        };
        assert_eq!(result["server_url"], "http://100.64.1.2:1234");
        assert_eq!(result["password_set"], true);
        assert_no_password_key(&result);
    }

    #[tokio::test]
    async fn config_reconnect_on_empty_store_is_unconfigured() {
        let (_dir, sock, _events, _link, _cache) = boot().await;
        let mut stream = UnixStream::connect(&sock).await.unwrap();
        write_req(&mut stream, "config.reconnect", json!({})).await;
        let env = read_res(stream).await;
        match env {
            Envelope::Res {
                ok: true, result, ..
            } => {
                let result = result.unwrap();
                assert_eq!(result["session"], "unconfigured");
                assert_eq!(result["password_set"], false);
                assert_no_password_key(&result);
            }
            other => panic!("expected config.reconnect res, got {other:?}"),
        }
    }
}
