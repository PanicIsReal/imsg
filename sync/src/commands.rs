use crate::cache::MessageCache;
use crate::config::default_socket_path;
use crate::link::{self, Link, Provision};
use crate::socket_server;
use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn run_daemon() -> Result<()> {
    let link = Link::boot()?;
    let cache = MessageCache::open(link.cache_path()).await?;
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(256);
    let cache = Arc::new(RwLock::new(cache));
    link.spawn_session(Arc::clone(&cache), event_tx.clone());
    socket_server::serve(cache, event_tx, link).await
}

pub async fn status() -> Result<SyncStatus> {
    let ctx = link::production_ctx();
    let provision = crate::link::store::load(&ctx).unwrap_or(Provision::Empty);
    let (cache_path, bridge_url) = match &provision {
        Provision::Ready(creds) => (
            creds.public.cache_path.clone(),
            creds.public.server.as_str().to_string(),
        ),
        Provision::Empty => (ctx.cache_path.clone(), String::new()),
    };
    let (chats, messages) = if cache_path.exists() {
        let cache = MessageCache::open(&cache_path).await?;
        (cache.chat_count().await?, cache.message_count().await?)
    } else {
        (0, 0)
    };
    Ok(SyncStatus {
        cache_path,
        bridge_url,
        chats,
        messages,
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

    let ctx = link::production_ctx();
    ok &= push("config", ctx.config_path.exists(), "config.toml");

    match crate::link::store::peek_server_url(&ctx) {
        Ok(Some(url)) => {
            ok &= push("server-url", true, url.as_str());
        }
        Ok(None) => {
            ok &= push("server-url", false, "not set");
        }
        Err(e) => {
            ok &= push("server-url", false, &e.to_string());
        }
    }

    match crate::link::store::password_set(&ctx) {
        Ok(true) => {
            ok &= push("password", true, "set");
        }
        Ok(false) => {
            ok &= push("password", false, "not set");
        }
        Err(e) => {
            ok &= push("password", false, &e.to_string());
        }
    }

    Ok(DoctorReport { ok, checks })
}

pub async fn request(method: &str, params: &str) -> Result<String> {
    let params: serde_json::Value = serde_json::from_str(params)?;
    let req = imsg_proto::Envelope::Req {
        id: "1".into(),
        method: method.into(),
        params,
    };
    let mut stream = tokio::net::UnixStream::connect(default_socket_path()).await?;
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
