use crate::cache::MessageCache;
use crate::client;
use crate::config::SyncConfig;
use crate::socket_server;
use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn run_daemon() -> Result<()> {
    let config = SyncConfig::load()?;
    let cache = MessageCache::open(&config.cache_path).await?;
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(256);
    let cache = Arc::new(RwLock::new(cache));
    let uplink = crate::uplink::UplinkHandle::default();

    let client_cache = Arc::clone(&cache);
    let client_config = config.clone();
    let client_tx = event_tx.clone();
    let client_uplink = uplink.clone();
    tokio::spawn(async move {
        if let Err(e) =
            client::bridge_loop(client_config, client_cache, client_tx, client_uplink).await
        {
            tracing::error!("bridge loop: {e}");
        }
    });

    socket_server::serve(config.socket_path, cache, event_tx, uplink).await
}

pub async fn status() -> Result<SyncStatus> {
    let config = SyncConfig::load()?;
    let cache = MessageCache::open(&config.cache_path).await?;
    Ok(SyncStatus {
        cache_path: config.cache_path.clone(),
        bridge_url: config.bridge_url.clone(),
        chats: cache.chat_count().await?,
        messages: cache.message_count().await?,
    })
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub cache_path: std::path::PathBuf,
    pub bridge_url: String,
    pub chats: i64,
    pub messages: i64,
}

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

pub fn doctor() -> Result<DoctorReport> {
    let config = SyncConfig::load()?;
    let mut checks = Vec::new();
    let mut ok = true;

    let mut push = |name: &str, pass: bool, detail: &str| {
        checks.push(DoctorCheck {
            name: name.into(),
            ok: pass,
            detail: detail.into(),
        });
        pass
    };

    ok &= push("config", SyncConfig::path().exists(), "config.toml");
    ok &= push("ca", config.ca_cert_path.exists(), "ca.pem");
    ok &= push(
        "client-cert",
        config.client_cert_path.exists(),
        "client.pem",
    );
    ok &= push(
        "client-key",
        config.client_key_path.exists(),
        "client-key.pem",
    );
    ok &= push(
        "bridge-url",
        !config.bridge_url.is_empty(),
        &config.bridge_url,
    );

    Ok(DoctorReport { ok, checks })
}

pub async fn request(method: &str, params: &str) -> Result<String> {
    let config = SyncConfig::load()?;
    let params: serde_json::Value = serde_json::from_str(params)?;
    let req = imsg_proto::Envelope::Req {
        id: "1".into(),
        method: method.into(),
        params,
    };
    let mut stream = tokio::net::UnixStream::connect(&config.socket_path).await?;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    stream
        .write_all(format!("{}\n", req.to_line()?).as_bytes())
        .await?;
    let mut lines = BufReader::new(stream).lines();
    if let Some(line) = lines.next_line().await? {
        return Ok(line);
    }
    anyhow::bail!("no response")
}
