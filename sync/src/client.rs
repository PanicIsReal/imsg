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
    let result = connect_and_sync_inner(config, cache, events).await;
    {
        let guard = cache.write().await;
        let _ = guard.set_meta("bridge_connected", "false").await;
    }
    result
}

async fn set_link_state(
    cache: &Arc<RwLock<MessageCache>>,
    bridge_connected: bool,
    database_ready: bool,
    last_error: &str,
) -> Result<()> {
    let guard = cache.write().await;
    guard
        .set_meta(
            "bridge_connected",
            if bridge_connected { "true" } else { "false" },
        )
        .await?;
    guard
        .set_meta(
            "database_ready",
            if database_ready { "true" } else { "false" },
        )
        .await?;
    guard.set_meta("last_error", last_error).await?;
    Ok(())
}

async fn prefetch_cache<W, R>(
    write: &mut W,
    read: &mut R,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
) -> Result<()>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let chats = rpc_call(
        write,
        read,
        "chats.list",
        json!({"limit": config.prefetch_chats}),
    )
    .await?;
    let list = chats
        .get("chats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for chat in list {
        {
            let guard = cache.write().await;
            guard.upsert_chat(&chat).await?;
        }
        let Some(id) = chat.get("id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let hist = rpc_call(
            write,
            read,
            "messages.history",
            json!({"chat_id": id, "limit": config.prefetch_messages}),
        )
        .await?;
        if let Some(msgs) = hist.get("messages").and_then(|v| v.as_array()) {
            let guard = cache.write().await;
            for msg in msgs {
                guard.upsert_message(msg).await?;
            }
        }
    }
    Ok(())
}

async fn connect_and_sync_inner(
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
    set_link_state(cache, true, false, "").await?;

    let (mut write, mut read) = ws.split();

    match prefetch_cache(&mut write, &mut read, config, cache).await {
        Ok(()) => set_link_state(cache, true, true, "").await?,
        Err(e) => {
            warn!("prefetch failed, staying connected: {e}");
            set_link_state(cache, true, false, &e.to_string()).await?;
        }
    }

    let mut retry = tokio::time::interval(std::time::Duration::from_secs(30));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(Envelope::Event { topic, payload }) = Envelope::parse_line(&text) {
                            if topic == "message" {
                                let guard = cache.write().await;
                                guard.upsert_message(&payload).await?;
                                let _ = events.send(Envelope::Event { topic, payload });
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            _ = retry.tick() => {
                let ready = cache
                    .read()
                    .await
                    .get_meta("database_ready")
                    .await?
                    .is_some_and(|v| v == "true");
                if !ready {
                    match prefetch_cache(&mut write, &mut read, config, cache).await {
                        Ok(()) => set_link_state(cache, true, true, "").await?,
                        Err(e) => {
                            warn!("prefetch retry failed: {e}");
                            set_link_state(cache, true, false, &e.to_string()).await?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub async fn bridge_request(config: &SyncConfig, method: &str, params: Value) -> Result<Value> {
    let tls = build_tls(config)?;
    let connector = Connector::Rustls(Arc::new(tls));
    let (ws, _) = connect_async_tls_with_config(&config.bridge_url, None, false, Some(connector))
        .await
        .context("connect bridge")?;
    let (mut write, mut read) = ws.split();
    rpc_call(&mut write, &mut read, method, params).await
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
