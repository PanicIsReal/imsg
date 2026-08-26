use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Mac,
    Linux,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub role: Role,
    pub hostname: String,
    pub imsg_binary: Option<PathBuf>,
    pub config_paths: ConfigPaths,
}

#[derive(Debug, Serialize)]
pub struct ConfigPaths {
    pub bridge_config: PathBuf,
    pub sync_config: PathBuf,
    pub sync_state: PathBuf,
    pub plugin_dir: PathBuf,
}

pub fn detect() -> PlatformInfo {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let role = match os.as_str() {
        "macos" => Role::Mac,
        "linux" => Role::Linux,
        _ => Role::Unknown,
    };
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "localhost".into());

    let sync_state = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("omarchy-imessage");

    PlatformInfo {
        os,
        arch,
        role,
        hostname,
        imsg_binary: which_imsg(),
        config_paths: ConfigPaths {
            bridge_config: dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("imsg-bridge")
                .join("config.toml"),
            sync_config: dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("imsg-sync")
                .join("config.toml"),
            sync_state,
            plugin_dir: dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("omarchy")
                .join("plugins")
                .join("io.github.panic.imessage"),
        },
    }
}

fn which_imsg() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

pub fn print(json: bool) -> Result<()> {
    let info = detect();
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("OS: {} ({})", info.os, info.arch);
        println!("Role: {:?}", info.role);
        println!("Hostname: {}", info.hostname);
        println!("Bridge config: {:?}", info.config_paths.bridge_config);
        println!("Sync config: {:?}", info.config_paths.sync_config);
    }
    Ok(())
}

pub fn tailscale_ip_hint() -> Option<String> {
    let output = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ip.starts_with("100.") {
        Some(ip)
    } else {
        None
    }
}

pub fn imsg_path() -> Result<PathBuf> {
    std::env::current_exe().context("resolve imsg binary path")
}
