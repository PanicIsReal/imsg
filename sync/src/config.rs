use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

#[derive(Clone)]
pub struct ServerUrl(Url);

impl ServerUrl {
    pub fn parse(raw: &str) -> Result<Self> {
        let url = Url::parse(raw.trim()).context("server_url")?;
        match url.scheme() {
            "http" | "https" => {}
            other => bail!("server_url must be http or https, not {other}"),
        }
        if url.host_str().is_none() {
            bail!("server_url needs a host");
        }
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    pub fn join(&self, path: &str) -> Result<Url> {
        self.0.join(path.trim_start_matches('/')).context("join server path")
    }
}

impl std::fmt::Debug for ServerUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone)]
pub struct Password(String);

impl Password {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let s = raw.into();
        if s.trim().is_empty() {
            bail!("password is empty");
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub server: ServerUrl,
    pub password: Password,
    pub cache_path: PathBuf,
    pub socket_path: PathBuf,
    pub prefetch_chats: u32,
    pub prefetch_messages: u32,
}

#[derive(Deserialize, Serialize)]
struct SyncConfigFile {
    server_url: String,
    password: String,
    #[serde(default)]
    cache_path: Option<PathBuf>,
    #[serde(default)]
    socket_path: Option<PathBuf>,
    #[serde(default)]
    prefetch_chats: Option<u32>,
    #[serde(default)]
    prefetch_messages: Option<u32>,
}

impl SyncConfig {
    pub fn from_parts(url: &str, password: &str) -> Result<Self> {
        let state = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omarchy-imessage");
        Ok(Self {
            server: ServerUrl::parse(url)?,
            password: Password::new(password)?,
            cache_path: state.join("cache.db"),
            socket_path: default_socket_path(),
            prefetch_chats: 20,
            prefetch_messages: 50,
        })
    }

    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("imsg-sync")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            bail!(
                "missing {}; run: imsg setup connect --url http://<mac>:1234 --password <password>",
                path.display()
            );
        }
        let text = std::fs::read_to_string(&path).context("read sync config")?;
        let file: SyncConfigFile = toml::from_str(&text).context("parse sync config")?;
        let mut cfg = Self::from_parts(&file.server_url, &file.password)?;
        if let Some(p) = file.cache_path {
            cfg.cache_path = p;
        }
        if let Some(p) = file.socket_path {
            cfg.socket_path = p;
        }
        if let Some(n) = file.prefetch_chats {
            cfg.prefetch_chats = n;
        }
        if let Some(n) = file.prefetch_messages {
            cfg.prefetch_messages = n;
        }
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = SyncConfigFile {
            server_url: self.server.as_str().to_string(),
            password: self.password.as_str().to_string(),
            cache_path: None,
            socket_path: None,
            prefetch_chats: Some(self.prefetch_chats),
            prefetch_messages: Some(self.prefetch_messages),
        };
        std::fs::write(&path, toml::to_string_pretty(&file)?).context("write sync config")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn default_socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/run/user/{uid}/imsg-sync.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wss_and_empty_password() {
        assert!(ServerUrl::parse("wss://mac:1234").is_err());
        assert!(Password::new("").is_err());
        assert!(ServerUrl::parse("http://100.64.1.2:1234").is_ok());
    }
}
