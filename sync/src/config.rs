use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub bridge_url: String,
    pub ca_cert_path: PathBuf,
    pub client_cert_path: PathBuf,
    pub client_key_path: PathBuf,
    pub cache_path: PathBuf,
    pub socket_path: PathBuf,
    pub prefetch_chats: u32,
    pub prefetch_messages: u32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        let state = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omarchy-imessage");
        Self {
            bridge_url: "wss://127.0.0.1:18789/ws".into(),
            ca_cert_path: state.join("ca.pem"),
            client_cert_path: state.join("client.pem"),
            client_key_path: state.join("client-key.pem"),
            cache_path: state.join("cache.db"),
            socket_path: default_socket_path(),
            prefetch_chats: 20,
            prefetch_messages: 50,
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/run/user/{uid}/imsg-sync.sock"))
}

impl SyncConfig {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("imsg-sync")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if path.exists() {
            let text = std::fs::read_to_string(&path).context("read sync config")?;
            Ok(toml::from_str(&text)?)
        } else {
            Ok(Self::default())
        }
    }
}
