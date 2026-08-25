use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    pub imsg_path: String,
    pub enable_send: bool,
    pub data_dir: PathBuf,
    pub pairing_code: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("imsg-bridge");
        Self {
            bind: "127.0.0.1".into(),
            port: 18789,
            imsg_path: "imsg".into(),
            enable_send: false,
            data_dir,
            pairing_code: None,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("imsg-bridge")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if path.exists() {
            let text = std::fs::read_to_string(&path).context("read config")?;
            toml::from_str(&text).context("parse config")
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn validate_bind(&self) -> Result<()> {
        let ip: IpAddr = self
            .bind
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid bind address: {}", self.bind))?;
        if ip.is_unspecified() {
            anyhow::bail!("refusing to bind to unspecified address (0.0.0.0)");
        }
        if ip.is_loopback() {
            return Ok(());
        }
        if is_tailscale_or_private(&ip) {
            return Ok(());
        }
        anyhow::bail!(
            "bind address {} must be loopback, Tailscale (100.64.0.0/10), or RFC1918",
            ip
        )
    }
}

fn is_tailscale_or_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
                || o[0] == 10
                || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
                || (o[0] == 192 && o[1] == 168)
        }
        IpAddr::V6(_) => false,
    }
}
