use crate::cache::MessageCache;
use crate::config::SyncConfig;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use imsg_proto::Envelope;
use rustls::{ClientConfig, RootCertStore};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{info, warn};

pub async fn bridge_loop(
    config: SyncConfig,
    cache: Arc<RwLock<MessageCache>>,
    events: tokio::sync::broadcast::Sender<Envelope>,
) -> Result<()> {
    loop {
        match connect_and_sync(&config, &cache, &events).await {
            Ok(()) => warn!("bridge connection closed, reconnecting"),
            Err(e) => warn!("bridge error: {e}, retry in 5s"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn connect_and_sync(
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &tokio::sync::broadcast::Sender<Envelope>,
) -> Result<()> {
    let tls = build_tls(config)?;
    let connector = Connector::Rustls(Arc::new(tls));
    let (ws, _) = connect_async_tls_with_config(&config.bridge_url, None, false, Some(connector))
        .await
        .context("connect bridge")?;
    info!("connected to bridge");

    let (mut write, mut read) = ws.split();

    let chats = rpc_call(&mut write, &mut read, "chats.list", json!({"limit": config.prefetch_chats})).await?;
    if let Some(list) = chats.get("chats").and_then(|v| v.as_array()) {
        let guard = cache.write().await;
        for chat in list {
            guard.upsert_chat(chat).await?;
            if let Some(id) = chat.get("id").and_then(|v| v.as_i64()) {
                let hist = rpc_call(
                    &mut write,
                    &mut read,
                    "messages.history",
                    json!({"chat_id": id, "limit": config.prefetch_messages}),
                )
                .await?;
                if let Some(msgs) = hist.get("messages").and_then(|v| v.as_array()) {
                    for msg in msgs {
                        guard.upsert_message(msg).await?;
                    }
                }
            }
        }
    }

    while let Some(msg) = read.next().await {
        match msg? {
            Message::Text(text) => {
                if let Ok(Envelope::Event { topic, payload }) = Envelope::parse_line(&text) {
                    if topic == "message" {
                        let guard = cache.write().await;
                        guard.upsert_message(&payload).await?;
                        let _ = events.send(Envelope::Event { topic, payload });
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

async fn rpc_call<W, R>(
    write: &mut W,
    read: &mut R,
    method: &str,
    params: Value,
) -> Result<Value>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string();
    let req = Envelope::Req {
        id: id.clone(),
        method: method.into(),
        params,
    };
    write.send(Message::Text(req.to_line()?.into())).await?;
    while let Some(msg) = read.next().await {
        let msg = msg?;
        if let Message::Text(text) = msg {
            if let Ok(Envelope::Res { id: rid, ok: true, result: Some(result), .. }) =
                Envelope::parse_line(&text)
            {
                if rid == id {
                    return Ok(result);
                }
            }
            if let Ok(Envelope::Res { id: rid, ok: false, error, .. }) =
                Envelope::parse_line(&text)
            {
                if rid == id {
                    anyhow::bail!("{:?}", error);
                }
            }
        }
    }
    anyhow::bail!("connection closed awaiting {method}")
}

fn build_tls(config: &SyncConfig) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    let ca = std::fs::read(&config.ca_cert_path).context("read ca")?;
    for cert in rustls_pemfile::certs(&mut ca.as_slice()) {
        roots.add(cert?).context("add ca")?;
    }
    let cert_pem = std::fs::read(&config.client_cert_path).context("read client cert")?;
    let key_pem = std::fs::read(&config.client_key_path).context("read client key")?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())?
        .ok_or_else(|| anyhow::anyhow!("no client key"))?;

    ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)
        .context("client tls")
}
