use crate::attachments;
use crate::config::Config;
use crate::contacts;
use crate::imsg_rpc::{bridge_method_to_imsg, envelope_error, envelope_ok, ImsgRpc};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use imsg_proto::{ContactsState, Envelope};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub rpc: Arc<ImsgRpc>,
    pub config: Config,
    pub db_generation: Arc<RwLock<String>>,
    pub events: broadcast::Sender<Envelope>,
    /// Probed at startup, updated after an authorize attempt.
    pub contacts: Arc<RwLock<ContactsState>>,
    /// One authorize attempt in flight at a time.
    pub contacts_gate: Arc<Mutex<()>>,
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

    let contacts_state = initial_contacts_state(&rpc, &config).await;
    let (events_tx, _) = broadcast::channel(256);
    let state = AppState {
        rpc: Arc::clone(&rpc),
        config: config.clone(),
        db_generation: Arc::new(RwLock::new(db_gen)),
        events: events_tx.clone(),
        contacts: Arc::new(RwLock::new(contacts_state)),
        contacts_gate: Arc::new(Mutex::new(())),
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

async fn initial_contacts_state(rpc: &ImsgRpc, config: &Config) -> ContactsState {
    let handle = rpc
        .call("chats.list", json!({"limit": 1}))
        .await
        .ok()
        .and_then(|v| contacts::first_handle(&v));
    let status = match contacts::probe_with_handle(config, handle.as_deref()).await {
        Ok(status) => status,
        Err(e) => {
            warn!("contacts probe: {e}");
            return ContactsState::Unavailable;
        }
    };
    let names = contacts::names_visible(rpc).await.unwrap_or(false);
    status.as_wire(names)
}

fn spawn_watch_forwarder(rpc: Arc<ImsgRpc>, events: broadcast::Sender<Envelope>) {
    tokio::spawn(async move {
        loop {
            match rpc.ensure_watch().await {
                Ok(()) => break,
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

    let contacts_state = *state.contacts.read().await;
    let contacts_event = Envelope::Event {
        topic: "contacts".into(),
        payload: json!({"state": contacts_state}),
    };
    if let Ok(line) = contacts_event.to_line() {
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
            if method == "contacts.status" {
                let contacts_state = *state.contacts.read().await;
                return Some(envelope_ok(&id, json!({"state": contacts_state})));
            }
            if method == "contacts.authorize" {
                return Some(handle_contacts_authorize(state, &id).await);
            }
            if !imsg_proto::Envelope::method_allowed(&method) && !state.config.enable_send {
                return Some(envelope_error(
                    &id,
                    "forbidden",
                    &format!("method not allowed: {method}"),
                ));
            }
            if method == "watch.ack" {
                return Some(envelope_ok(&id, json!({"ok": true})));
            }
            if method == "attachments.fetch" {
                let chat_guid = params
                    .get("chat_guid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let message_guid = params
                    .get("message_guid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let filename = params
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let secret = state.config.data_dir.to_string_lossy().into_owned();
                let token =
                    attachments::token_for(chat_guid, message_guid, filename, secret.as_bytes());
                return Some(envelope_ok(&id, json!({"token": token, "expires_in": 300})));
            }
            if let Some(imsg_method) = bridge_method_to_imsg(&method) {
                match state.rpc.call(imsg_method, params).await {
                    Ok(result) => Some(envelope_ok(&id, result)),
                    Err(e) => Some(envelope_error(&id, "upstream_error", &e.to_string())),
                }
            } else {
                Some(envelope_error(
                    &id,
                    "unknown_method",
                    &format!("unknown: {method}"),
                ))
            }
        }
        _ => None,
    }
}

const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(110);

async fn handle_contacts_authorize(state: &AppState, id: &str) -> Envelope {
    let Some(_gate) = contacts::try_lock_gate(&state.contacts_gate) else {
        return envelope_ok(id, contacts::busy_gate_reply());
    };
    store_and_publish_contacts(state, ContactsState::Prompting).await;
    let outcome = match contacts::authorize(&state.config, &state.rpc, AUTHORIZE_TIMEOUT).await {
        Ok(outcome) => outcome,
        Err(e) => {
            warn!("contacts authorize: {e}");
            contacts::ContactsOutcome::HelperMissing {
                detail: e.to_string(),
            }
        }
    };
    store_and_publish_contacts(state, outcome.as_state()).await;
    match serde_json::to_value(&outcome) {
        Ok(v) => envelope_ok(id, v),
        Err(e) => envelope_error(id, "error", &e.to_string()),
    }
}

async fn store_and_publish_contacts(state: &AppState, contacts_state: ContactsState) {
    *state.contacts.write().await = contacts_state;
    let _ = state.events.send(Envelope::Event {
        topic: "contacts".into(),
        payload: json!({"state": contacts_state}),
    });
}
