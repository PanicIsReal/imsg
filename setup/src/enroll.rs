use crate::paths::{default_client_name, default_socket_path, SyncPaths};
use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct EnrollResponse {
    ca_pem: String,
    client_cert_pem: String,
    bridge_url: String,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize)]
pub struct PairResult {
    pub client_name: String,
    pub config_path: PathBuf,
    pub bridge_url: String,
}

pub async fn pair(
    code: &str,
    host: &str,
    enroll_port: u16,
    name: Option<&str>,
    insecure: bool,
) -> Result<PairResult> {
    let default_name = default_client_name();
    let client_name = name.unwrap_or(&default_name).to_string();
    let paths = SyncPaths::default_paths();
    paths.ensure_state_dir()?;

    let alg = &PKCS_ECDSA_P256_SHA256;
    let key_pair = KeyPair::generate_for(alg)?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, &client_name);
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ClientAuth);
    let csr = params.serialize_request(&key_pair)?;
    let csr_pem = csr.pem().map_err(|e| anyhow::anyhow!("csr pem: {e}"))?;

    let url = format!("https://{host}:{enroll_port}/v1/pair/enroll");

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure)
        .build()?;

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "pairing_code": code.trim(),
            "client_name": client_name,
            "csr_pem": csr_pem,
        }))
        .send()
        .await
        .context("enroll request")?;

    let status = resp.status();
    if !status.is_success() {
        let err: ErrorResponse = resp.json().await.unwrap_or(ErrorResponse {
            error: format!("HTTP {status}"),
        });
        anyhow::bail!("pairing failed: {}", err.error);
    }

    let body: EnrollResponse = resp.json().await.context("parse enroll response")?;

    fs::write(paths.state_dir.join("ca.pem"), &body.ca_pem)?;
    fs::write(paths.state_dir.join("client.pem"), &body.client_cert_pem)?;
    fs::write(
        paths.state_dir.join("client-key.pem"),
        key_pair.serialize_pem(),
    )?;

    let config = format!(
        r#"bridge_url = "{bridge_url}"
ca_cert_path = "{ca}"
client_cert_path = "{cert}"
client_key_path = "{key}"
cache_path = "{cache}"
socket_path = "{socket}"
prefetch_chats = 20
prefetch_messages = 50
"#,
        bridge_url = body.bridge_url,
        ca = paths.state_dir.join("ca.pem").display(),
        cert = paths.state_dir.join("client.pem").display(),
        key = paths.state_dir.join("client-key.pem").display(),
        cache = paths.state_dir.join("cache.db").display(),
        socket = default_socket_path().display(),
    );
    fs::write(&paths.config_path, config)?;

    Ok(PairResult {
        client_name,
        config_path: paths.config_path,
        bridge_url: body.bridge_url,
    })
}

pub async fn pair_human(
    code: &str,
    host: &str,
    enroll_port: u16,
    name: Option<&str>,
    insecure: bool,
) -> Result<()> {
    let url = format!("https://{host}:{enroll_port}/v1/pair/enroll");
    println!("Enrolling with {url} ...");
    let result = pair(code, host, enroll_port, name, insecure).await?;
    println!("Paired successfully as '{}'", result.client_name);
    println!("Config: {:?}", result.config_path);
    println!("Bridge: {}", result.bridge_url);
    println!("Next: imsg sync run");
    Ok(())
}
