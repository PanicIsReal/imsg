use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct SyncPaths {
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
}

impl SyncPaths {
    pub fn default_paths() -> Self {
        let state_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omarchy-imessage");
        let config_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("imsg-sync")
            .join("config.toml");
        Self {
            state_dir,
            config_path,
        }
    }

    pub fn ensure_state_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state_dir).context("create state dir")?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).context("create config dir")?;
        }
        Ok(())
    }
}

pub fn default_socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/run/user/{uid}/imsg-sync.sock"))
}

pub fn default_client_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "omarchy-client".into())
}
