use crate::config::Config;
use crate::imsg_rpc::ImsgRpc;
use crate::pairing;
use crate::server;
use crate::tls;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct InitResult {
    pub data_dir: PathBuf,
    pub pairing_code: String,
    pub enroll_port: u16,
    pub mdns_advertise: bool,
}

pub fn init(bind: String, port: u16, mdns: bool) -> Result<InitResult> {
    let mut config = Config::load().unwrap_or_default();
    config.bind = bind.clone();
    config.port = port;
    config.enroll_port = port.saturating_add(1);
    config.mdns_advertise = mdns;
    if config.pairing_code.is_none() || !pairing::pairing_code_valid(&config) {
        pairing::rotate_pairing_code(&mut config);
    }
    config.save()?;
    tls::init_certs(&config.data_dir, &bind)?;
    Ok(InitResult {
        data_dir: config.data_dir.clone(),
        pairing_code: config.pairing_code.clone().unwrap_or_default(),
        enroll_port: config.enroll_port,
        mdns_advertise: mdns,
    })
}

pub fn pair(json: bool, rotate: bool) -> Result<Option<serde_json::Value>> {
    let mut config = Config::load()?;
    if rotate {
        pairing::rotate_pairing_code(&mut config);
        config.save()?;
    }
    let status = pairing::pairing_status(&config);
    if json {
        return Ok(Some(serde_json::to_value(status)?));
    }
    println!("CA cert: {:?}", status.ca_path);
    println!(
        "Pairing code: {}",
        status.code.as_deref().unwrap_or("(run init first)")
    );
    if let Some(exp) = status.expires_at {
        println!("Expires: {exp}");
    }
    println!("Enroll URL: {}", status.enroll_url);
    println!("On Linux: imsg setup pair <code> --host <mac-host>");
    Ok(None)
}

pub fn import_client(name: String, cert: PathBuf) -> Result<PathBuf> {
    let config = Config::load()?;
    let pem = std::fs::read_to_string(&cert).context("read client cert")?;
    tls::import_client_cert(&config.data_dir, &name, &pem)
}

pub async fn serve() -> Result<()> {
    let config = Config::load()?;
    config.validate_bind()?;
    server::run(config).await
}

pub async fn status() -> Result<serde_json::Value> {
    let config = Config::load().unwrap_or_default();
    let rpc = ImsgRpc::spawn(&config.imsg_path).await?;
    match rpc.status().await {
        Ok(status) => Ok(status),
        Err(_) => rpc.call("chats.list", serde_json::json!({"limit": 1})).await,
    }
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
    let config = Config::load().unwrap_or_default();
    let mut checks = Vec::new();

    let mut push = |name: &str, ok: bool, detail: &str| {
        checks.push(DoctorCheck {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
        ok
    };

    let mut ok = true;
    ok &= push(
        "config",
        Config::path().exists(),
        &format!("{:?}", Config::path()),
    );
    ok &= push(
        "ca",
        config.data_dir.join("ca.pem").exists(),
        "ca.pem present",
    );
    ok &= push(
        "server-tls",
        config.data_dir.join("server.pem").exists()
            && config.data_dir.join("server-key.pem").exists(),
        "server cert/key present",
    );
    ok &= push(
        "pairing",
        pairing::pairing_code_valid(&config),
        "pairing code active",
    );
    ok &= push(
        "bind",
        config.validate_bind().is_ok(),
        &format!("{}:{}", config.bind, config.port),
    );
    match crate::steipete::resolve_steipete_imsg(&config.imsg_path) {
        Some(path) => {
            ok &= push("steipete-imsg", true, &path.display().to_string());
        }
        None => {
            ok &= push("steipete-imsg", false, crate::steipete::INSTALL_CMD);
        }
    }

    Ok(DoctorReport { ok, checks })
}
