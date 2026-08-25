use crate::cache::MessageCache;
use anyhow::{Context, Result};
use imsg_proto::Envelope;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, RwLock};
use tracing::info;

pub async fn serve(
    socket_path: impl AsRef<Path>,
    cache: Arc<RwLock<MessageCache>>,
    mut events: broadcast::Receiver<Envelope>,
) -> Result<()> {
    let socket_path = socket_path.as_ref();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).context("bind unix socket")?;
    info!("imsg-sync socket at {:?}", socket_path);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let cache = Arc::clone(&cache);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, cache).await {
                        tracing::warn!("client error: {e}");
                    }
                });
            }
            _ = events.recv() => {}
        }
    }
}

async fn handle_client(stream: UnixStream, cache: Arc<RwLock<MessageCache>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let env = Envelope::parse_line(line)?;
        if let Envelope::Req { id, method, params } = env {
            let result = dispatch(&cache, &method, params).await;
            let reply = match result {
                Ok(v) => Envelope::Res {
                    id,
                    ok: true,
                    result: Some(v),
                    error: None,
                },
                Err(e) => Envelope::Res {
                    id,
                    ok: false,
                    result: None,
                    error: Some(imsg_proto::ErrorBody {
                        code: "error".into(),
                        message: e.to_string(),
                    }),
                },
            };
            writer.write_all(format!("{}\n", reply.to_line()?).as_bytes()).await?;
        }
    }
    Ok(())
}

async fn dispatch(cache: &Arc<RwLock<MessageCache>>, method: &str, params: Value) -> Result<Value> {
    let guard = cache.read().await;
    match method {
        "status" => Ok(json!({"connected": true, "protocol": imsg_proto::PROTOCOL_VERSION})),
        "chats.list" => {
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let chats = guard.list_chats(limit).await?;
            Ok(json!({"chats": chats}))
        }
        "messages.history" => {
            let chat_id = params["chat_id"].as_i64().context("chat_id required")?;
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let before = params.get("before").and_then(|v| v.as_str());
            let messages = guard.list_messages(chat_id, limit, before).await?;
            Ok(json!({"messages": messages}))
        }
        "messages.search" => {
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
