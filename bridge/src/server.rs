use crate::attachments;
use crate::config::Config;
use crate::imsg_rpc::{bridge_method_to_imsg, envelope_error, envelope_ok, ImsgRpc};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use imsg_proto::Envelope;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub rpc: Arc<ImsgRpc>,
    pub config: Config,
    pub db_generation: Arc<RwLock<String>>,
    pub events: broadcast::Sender<Envelope>,
}

pub async fn run(config: Config, tls: RustlsConfig) -> Result<()> {
    config.validate_bind()?;
    let addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;

    let rpc = ImsgRpc::spawn(&config.imsg_path).await?;
    let status = rpc.status().await.unwrap_or(json!({}));
    let db_gen = status
        .pointer("/database/path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let (events_tx, _) = broadcast::channel(256);
    let state = AppState {
        rpc: Arc::clone(&rpc),
        config: config.clone(),
        db_generation: Arc::new(RwLock::new(db_gen)),
        events: events_tx.clone(),
    };

    spawn_watch_forwarder(rpc, events_tx);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);

    info!("imsg-bridge listening on wss://{addr}");
    axum_server::bind_rustls(addr, tls)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

fn spawn_watch_forwarder(rpc: Arc<ImsgRpc>, events: broadcast::Sender<Envelope>) {
    tokio::spawn(async move {
        loop {
            match rpc
                .call("watch.subscribe", json!({"debounce_ms": 500}))
                .await
            {
                Ok(_) => {
                    info!("watch subscription active");
                    break;
                }
                Err(e) => {
                    warn!("watch.subscribe failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
        let mut rx = rpc.subscribe_events();
        while let Ok(msg) = rx.recv().await {
            let env = Envelope::Event {
                topic: "message".into(),
                payload: msg,
            };
            let _ = events.send(env);
        }
    });
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = state.events.subscribe();

    let db_gen = state.db_generation.read().await.clone();
    let gen_event = Envelope::Event {
        topic: "db.generation".into(),
        payload: json!({"generation": db_gen, "at": chrono::Utc::now().to_rfc3339()}),
    };
    if let Ok(line) = gen_event.to_line() {
        let _ = sender.send(Message::Text(line.into())).await;
    }

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(env) = Envelope::parse_line(&text) {
                            if let Some(reply) = handle_envelope(&state, env).await {
                                if let Ok(line) = reply.to_line() {
                                    let _ = sender.send(Message::Text(line.into())).await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = sender.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            evt = event_rx.recv() => {
                if let Ok(env) = evt {
                    if let Ok(line) = env.to_line() {
                        let _ = sender.send(Message::Text(line.into())).await;
                    }
                }
            }
        }
    }
}

async fn handle_envelope(state: &AppState, env: Envelope) -> Option<Envelope> {
    match env {
        Envelope::Ping => Some(Envelope::Pong),
        Envelope::Pong => None,
        Envelope::Req { id, method, params } => {
            if !imsg_proto::Envelope::method_allowed(&method) && !state.config.enable_send {
                return Some(envelope_error(&id, "forbidden", &format!("method not allowed: {method}")));
            }
            if method == "watch.ack" {
                return Some(envelope_ok(&id, json!({"ok": true})));
            }
            if method == "attachments.fetch" {
                let chat_guid = params.get("chat_guid").and_then(|v| v.as_str()).unwrap_or("");
                let message_guid = params.get("message_guid").and_then(|v| v.as_str()).unwrap_or("");
                let filename = params.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                let secret = state.config.data_dir.to_string_lossy().into_owned();
                let token = attachments::token_for(chat_guid, message_guid, filename, secret.as_bytes());
                return Some(envelope_ok(
                    &id,
                    json!({"token": token, "expires_in": 300}),
                ));
            }
            if let Some(imsg_method) = bridge_method_to_imsg(&method) {
                match state.rpc.call(imsg_method, params).await {
                    Ok(result) => Some(envelope_ok(&id, result)),
                    Err(e) => Some(envelope_error(&id, "upstream_error", &e.to_string())),
                }
            } else {
                Some(envelope_error(&id, "unknown_method", &format!("unknown: {method}")))
            }
        }
        _ => None,
    }
}
