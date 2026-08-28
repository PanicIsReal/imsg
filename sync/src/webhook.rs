use anyhow::{bail, Context, Result};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 18792;
pub const HOOK_PATH: &str = "/imsg/hook";
const MAX_BODY: usize = 64 * 1024;
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_MAX: u32 = 40;

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub kind: String,
    pub message_guid: Option<String>,
}

#[derive(Clone)]
struct HookState {
    token: String,
    events: mpsc::Sender<HookEvent>,
    hits: Arc<Mutex<(Instant, u32)>>,
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

pub fn generate_token() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn hook_url(serve_origin: &str, token: &str) -> Result<String> {
    let origin = serve_origin.trim().trim_end_matches('/');
    if origin.is_empty() {
        bail!("serve URL is empty");
    }
    let mut url = url::Url::parse(origin).context("serve URL")?;
    match url.scheme() {
        "http" | "https" => {}
        other => bail!("serve URL must be http or https, not {other}"),
    }
    url.set_path(HOOK_PATH.trim_start_matches('/'));
    url.set_query(None);
    url.query_pairs_mut().append_pair("token", token);
    Ok(url.to_string())
}

pub fn extract_guid(body: &Value) -> Option<String> {
    let data = body.get("data").unwrap_or(body);
    as_guid(data.get("guid"))
        .or_else(|| as_guid(data.get("message").and_then(|m| m.get("guid"))))
        .or_else(|| {
            data.get("messages")
                .and_then(|m| m.as_array())
                .and_then(|a| a.first())
                .and_then(|m| as_guid(m.get("guid")))
        })
}

fn as_guid(v: Option<&Value>) -> Option<String> {
    v.and_then(|g| g.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn event_kind(body: &Value) -> String {
    body.get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn token_matches(got: &str, want: &str) -> bool {
    let a = got.as_bytes();
    let b = want.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub async fn bind_local(port: u16) -> Result<(TcpListener, SocketAddr)> {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = TcpListener::bind(addr).await.context("bind webhook")?;
    let local = listener.local_addr().context("webhook local addr")?;
    if !local.ip().is_loopback() {
        bail!("webhook refused non-loopback bind {}", local.ip());
    }
    Ok((listener, local))
}

pub async fn serve(
    listener: TcpListener,
    token: String,
    events: mpsc::Sender<HookEvent>,
) -> Result<()> {
    let state = HookState {
        token,
        events,
        hits: Arc::new(Mutex::new((Instant::now(), 0))),
    };
    let app = Router::new()
        .route(HOOK_PATH, post(handle_hook))
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state);
    axum::serve(listener, app)
        .await
        .context("webhook serve")?;
    Ok(())
}

async fn method_not_allowed() -> impl IntoResponse {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle_hook(
    State(state): State<HookState>,
    method: Method,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if method != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED;
    }
    if !token_matches(query.token.as_deref().unwrap_or(""), &state.token) {
        return StatusCode::UNAUTHORIZED;
    }
    if !rate_ok(&state.hits).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    if body.len() > MAX_BODY {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    let ctype = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ctype.to_ascii_lowercase().contains("json") && !body.is_empty() {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE;
    }
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    let kind = event_kind(&parsed);
    if kind.is_empty() {
        return StatusCode::NO_CONTENT;
    }
    let guid = if kind == "new-message" || kind == "updated-message" {
        extract_guid(&parsed)
    } else {
        None
    };
    let _ = state.events.try_send(HookEvent {
        kind,
        message_guid: guid,
    });
    StatusCode::NO_CONTENT
}

async fn rate_ok(hits: &Mutex<(Instant, u32)>) -> bool {
    let mut g = hits.lock().await;
    let now = Instant::now();
    if now.duration_since(g.0) > RATE_WINDOW {
        *g = (now, 1);
        return true;
    }
    if g.1 >= RATE_MAX {
        return false;
    }
    g.1 += 1;
    true
}

pub fn guess_serve_origin() -> Option<String> {
    let out = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let dns = v
        .get("Self")
        .and_then(|s| s.get("DNSName"))
        .and_then(|d| d.as_str())?
        .trim()
        .trim_end_matches('.');
    if dns.is_empty() {
        return None;
    }
    Some(format!("https://{dns}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_guid_from_nested_data() {
        let body = json!({"type": "new-message", "data": {"guid": "ABC"}});
        assert_eq!(extract_guid(&body).as_deref(), Some("ABC"));
        let inner = json!({"type": "updated-message", "data": {"message": {"guid": "Z"}}});
        assert_eq!(extract_guid(&inner).as_deref(), Some("Z"));
        assert!(extract_guid(&json!({"type": "hello-world"})).is_none());
    }

    #[test]
    fn hook_url_puts_token_on_query() {
        let url = hook_url("https://box.tailnet.ts.net", "abc").unwrap();
        assert!(url.starts_with("https://box.tailnet.ts.net/imsg/hook?"));
        assert!(url.contains("token=abc"));
        assert!(!url.contains("password"));
    }

    #[test]
    fn token_matches_rejects_wrong_and_short() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("", "abcd"));
    }

    #[tokio::test]
    async fn bind_is_loopback() {
        let (listener, addr) = bind_local(0).await.unwrap();
        assert!(addr.ip().is_loopback());
        drop(listener);
    }

    async fn spawn_server() -> (SocketAddr, mpsc::Receiver<HookEvent>) {
        let (listener, addr) = bind_local(0).await.unwrap();
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = serve(listener, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(), tx).await;
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        (addr, rx)
    }

    #[tokio::test]
    async fn missing_token_is_401() {
        let (addr, _rx) = spawn_server().await;
        let url = format!("http://{addr}{HOOK_PATH}");
        let res = reqwest::Client::new()
            .post(url)
            .header("content-type", "application/json")
            .body(r#"{"type":"new-message","data":{"guid":"g"}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_is_405() {
        let (addr, _rx) = spawn_server().await;
        let url = format!("http://{addr}{HOOK_PATH}?token=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let res = reqwest::Client::new().get(url).send().await.unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn good_token_enqueues_guid() {
        let (addr, mut rx) = spawn_server().await;
        let url = format!("http://{addr}{HOOK_PATH}?token=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let res = reqwest::Client::new()
            .post(url)
            .header("content-type", "application/json")
            .body(r#"{"type":"new-message","data":{"guid":"MSG-1"}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::NO_CONTENT);
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.kind, "new-message");
        assert_eq!(ev.message_guid.as_deref(), Some("MSG-1"));
    }
}
