use crate::config::Config;
use crate::tls;
use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{info, warn};

const MAX_ATTEMPTS_PER_IP: u32 = 5;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(300);
const CODE_TTL: Duration = Duration::from_secs(900);

#[derive(Clone)]
pub struct PairingState {
    pub config: Config,
    attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    pub pairing_code: String,
    pub client_name: String,
    pub csr_pem: String,
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub bridge_url: String,
    pub bridge_host: String,
    pub bridge_port: u16,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub fn pairing_code_valid(config: &Config) -> bool {
    let Some(code) = &config.pairing_code else {
        return false;
    };
    if code.is_empty() {
        return false;
    }
    if let Some(expires) = config.pairing_code_expires_at {
        return Utc::now() < expires;
    }
    true
}

/// Enroll must read the ticket from disk. The listen config is a clone from serve start.
pub fn merge_live_ticket(listen: &Config, disk: &Config) -> Config {
    let mut live = listen.clone();
    live.pairing_code = disk.pairing_code.clone();
    live.pairing_code_expires_at = disk.pairing_code_expires_at;
    live
}

fn live_pairing_config(listen: &Config) -> Config {
    match Config::load() {
        Ok(disk) => merge_live_ticket(listen, &disk),
        Err(_) => listen.clone(),
    }
}

pub fn rotate_pairing_code(config: &mut Config) -> String {
    let code = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    config.pairing_code = Some(code.clone());
    config.pairing_code_expires_at =
        Some(Utc::now() + chrono::Duration::seconds(CODE_TTL.as_secs() as i64));
    code
}

pub async fn run_enroll_server(config: Config, tls: RustlsConfig) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.bind, config.enroll_port).parse()?;
    let state = PairingState {
        config: config.clone(),
        attempts: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/v1/pair/enroll", post(enroll_handler))
        .with_state(state);

    info!("pairing enrollment listening on https://{addr}/v1/pair/enroll");
    axum_server::bind_rustls(addr, tls)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}

async fn enroll_handler(
    State(state): State<PairingState>,
    connect_info: axum::extract::ConnectInfo<SocketAddr>,
    Json(req): Json<EnrollRequest>,
) -> impl IntoResponse {
    let peer = connect_info.0.ip().to_string();
    if !rate_limit_ok(&state, &peer).await {
        warn!("pairing rate limit exceeded for {peer}");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "too many pairing attempts".into(),
            }),
        )
            .into_response();
    }

    let ticket = live_pairing_config(&state.config);
    if !pairing_code_valid(&ticket) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "pairing code expired or not configured. Run imsg setup on the Mac.".into(),
            }),
        )
            .into_response();
    }

    let expected = ticket.pairing_code.as_deref().unwrap_or("");
    if !constant_time_eq(expected, req.pairing_code.trim()) {
        record_attempt(&state, &peer).await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "invalid pairing code".into(),
            }),
        )
            .into_response();
    }

    let client_name = sanitize_client_name(&req.client_name);
    if client_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid client_name".into(),
            }),
        )
            .into_response();
    }

    match tls::sign_client_csr(&state.config.data_dir, &client_name, &req.csr_pem) {
        Ok(client_cert_pem) => {
            let ca_pem = match std::fs::read_to_string(state.config.data_dir.join("ca.pem")) {
                Ok(pem) => pem,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("read ca: {e}"),
                        }),
                    )
                        .into_response();
                }
            };

            let bridge_host = bridge_hostname(&state.config);
            let bridge_url = format!("wss://{}:{}/ws", bridge_host, state.config.port);

            info!("paired client '{client_name}' from {peer}");
            (
                StatusCode::OK,
                Json(EnrollResponse {
                    ca_pem,
                    client_cert_pem,
                    bridge_url,
                    bridge_host,
                    bridge_port: state.config.port,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("pairing enroll failed for {peer}: {e}");
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        }
    }
}

fn bridge_hostname(config: &Config) -> String {
    if config.bind != "127.0.0.1" && config.bind != "::1" {
        return config.bind.clone();
    }
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "localhost".into())
}

fn sanitize_client_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

async fn rate_limit_ok(state: &PairingState, peer: &str) -> bool {
    let mut map = state.attempts.lock().await;
    let now = Instant::now();
    let attempts = map.entry(peer.to_string()).or_default();
    attempts.retain(|t| now.duration_since(*t) < ATTEMPT_WINDOW);
    attempts.len() < MAX_ATTEMPTS_PER_IP as usize
}

async fn record_attempt(state: &PairingState, peer: &str) {
    let mut map = state.attempts.lock().await;
    let now = Instant::now();
    let attempts = map.entry(peer.to_string()).or_default();
    attempts.retain(|t| now.duration_since(*t) < ATTEMPT_WINDOW);
    attempts.push(now);
}

pub fn pairing_status(config: &Config) -> PairingStatus {
    PairingStatus {
        code: config.pairing_code.clone(),
        expires_at: config.pairing_code_expires_at,
        valid: pairing_code_valid(config),
        enroll_url: format!(
            "https://{}:{}/v1/pair/enroll",
            bridge_hostname(config),
            config.enroll_port
        ),
        ca_path: config.data_dir.join("ca.pem"),
    }
}

#[derive(Debug, Serialize)]
pub struct PairingStatus {
    pub code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub valid: bool,
    pub enroll_url: String,
    pub ca_path: std::path::PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_invalid() {
        assert_eq!(sanitize_client_name("omarchy-laptop_1"), "omarchy-laptop_1");
        assert_eq!(sanitize_client_name("bad name!"), "badname");
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abcd1234", "abcd1234"));
        assert!(!constant_time_eq("abcd1234", "abcd1235"));
        assert!(!constant_time_eq("short", "longer"));
    }

    #[test]
    fn enroll_uses_disk_ticket_not_serve_clone() {
        let mut listen = Config::default();
        listen.pairing_code = Some("oldcode1".into());
        listen.pairing_code_expires_at = Some(Utc::now() - chrono::Duration::seconds(60));
        let mut disk = listen.clone();
        disk.pairing_code = Some("newcode2".into());
        disk.pairing_code_expires_at = Some(Utc::now() + chrono::Duration::seconds(900));
        let live = merge_live_ticket(&listen, &disk);
        assert_eq!(live.pairing_code.as_deref(), Some("newcode2"));
        assert!(pairing_code_valid(&live));
        assert!(!pairing_code_valid(&listen));
    }
}
