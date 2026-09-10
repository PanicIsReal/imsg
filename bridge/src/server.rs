use crate::attachments;
use crate::config::Config;
use crate::contacts;
use crate::imsg_rpc::{bridge_method_to_imsg, envelope_error, envelope_ok, ImsgRpc, RpcEvent};
use crate::mdns_advertise;
use crate::pairing;
use crate::tls;
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use imsg_proto::{ContactsState, Envelope};
use serde_json::json;
use std::net::SocketAddr;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinSet;
use tracing::{info, warn};

const WS_SEND_TIMEOUT: Duration = Duration::from_secs(5);

fn database_generation(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!(
        "{}:{}:{}:{:?}",
        path,
        metadata.dev(),
        metadata.ino(),
        metadata.created().ok()
    ))
}

#[derive(Clone)]
pub struct AppState {
    pub rpc: Arc<ImsgRpc>,
    pub config: Config,
    pub db_generation: Arc<RwLock<String>>,
    pub database_path: String,
    pub events: broadcast::Sender<Envelope>,
    /// Probed at startup, updated after an authorize attempt.
    pub contacts: Arc<RwLock<ContactsState>>,
    /// One authorize attempt in flight at a time.
    pub contacts_gate: Arc<Mutex<()>>,
}

pub async fn run(config: Config) -> Result<()> {
    config.validate_bind()?;
    let addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;

    let tls_mat = tls::load_server_config(&config.data_dir)?;
    let wss_tls = axum_server::tls_rustls::RustlsConfig::from_config(tls_mat.server_config.clone());
    let enroll_tls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(
        tls::load_enroll_tls_config(&config.data_dir)?,
    ));

    mdns_advertise::spawn_if_enabled(&config)?;

    let enroll_config = config.clone();
    tokio::spawn(async move {
        if let Err(e) = pairing::run_enroll_server(enroll_config, enroll_tls).await {
            warn!("enroll server: {e}");
        }
    });

    let rpc = ImsgRpc::spawn(&config.imsg_path).await?;
    let status = rpc.status().await.unwrap_or(json!({}));
    let database_path = status
        .pointer("/database/path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let db_gen = database_generation(&database_path).unwrap_or_else(|| database_path.clone());

    let contacts_state = initial_contacts_state(&rpc, &config).await;
    let (events_tx, _) = broadcast::channel(256);
    let state = AppState {
        rpc: Arc::clone(&rpc),
        config: config.clone(),
        db_generation: Arc::new(RwLock::new(db_gen)),
        database_path,
        events: events_tx.clone(),
        contacts: Arc::new(RwLock::new(contacts_state)),
        contacts_gate: Arc::new(Mutex::new(())),
    };

    spawn_watch_forwarder(rpc, events_tx);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);

    info!("imsg-bridge listening on wss://{addr}");
    axum_server::bind_rustls(addr, wss_tls)
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
        let mut rx = rpc.subscribe_events();
        loop {
            match rpc.ensure_watch().await {
                Ok(()) => break,
                Err(e) => {
                    warn!("watch.subscribe failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
        loop {
            let envelope = match rx.recv().await {
                Ok(RpcEvent::Message(message)) => Envelope::Event {
                    topic: "message".into(),
                    payload: message,
                },
                Ok(RpcEvent::Gap { reason }) => {
                    while let Err(error) = rpc.ensure_watch().await {
                        warn!(%error, "watch resubscribe after rpc restart failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    watch_gap(reason)
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "imsg rpc event receiver lagged");
                    watch_gap("rpc_events_lagged")
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let _ = events.send(envelope);
        }
    });
}

fn watch_gap(reason: &str) -> Envelope {
    Envelope::Event {
        topic: "watch.gap".into(),
        payload: json!({"reason": reason}),
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    const MAX_IN_FLIGHT: usize = 32;
    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = state.events.subscribe();
    let mut requests = JoinSet::new();

    if let Some(generation) = database_generation(&state.database_path) {
        *state.db_generation.write().await = generation;
    }
    let db_gen = state.db_generation.read().await.clone();
    let gen_event = Envelope::Event {
        topic: "db.generation".into(),
        payload: json!({"generation": db_gen, "at": chrono::Utc::now().to_rfc3339()}),
    };
    if let Ok(line) = gen_event.to_line() {
        if !send_ws(&mut sender, Message::Text(line.into())).await {
            return;
        }
    }

    let contacts_state = *state.contacts.read().await;
    let contacts_event = Envelope::Event {
        topic: "contacts".into(),
        payload: json!({"state": contacts_state}),
    };
    if let Ok(line) = contacts_event.to_line() {
        if !send_ws(&mut sender, Message::Text(line.into())).await {
            return;
        }
    }

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(env) = Envelope::parse_line(&text) {
                            match env {
                                Envelope::Ping => {
                                    if let Ok(line) = Envelope::Pong.to_line() {
                                        if !send_ws(&mut sender, Message::Text(line.into())).await {
                                            break;
                                        }
                                    }
                                }
                                Envelope::Req { ref id, .. } if requests.len() >= MAX_IN_FLIGHT => {
                                    let reply = envelope_error(id, "busy", "too many requests in flight");
                                    if let Ok(line) = reply.to_line() {
                                        if !send_ws(&mut sender, Message::Text(line.into())).await {
                                            break;
                                        }
                                    }
                                }
                                _ => {
                                    let request_state = state.clone();
                                    requests.spawn(async move { handle_envelope(&request_state, env).await });
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if !send_ws(&mut sender, Message::Pong(p)).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            evt = event_rx.recv() => {
                let env = match evt {
                    Ok(env) => Some(env),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "websocket event receiver lagged");
                        Some(watch_gap("client_events_lagged"))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                };
                if let Some(Ok(line)) = env.map(|envelope| envelope.to_line()) {
                    if !send_ws(&mut sender, Message::Text(line.into())).await {
                        break;
                    }
                }
            }
            completed = requests.join_next(), if !requests.is_empty() => {
                if let Some(Ok(Some(reply))) = completed {
                    if let Ok(line) = reply.to_line() {
                        if !send_ws(&mut sender, Message::Text(line.into())).await {
                            break;
                        }
                    }
                }
            }
        }
    }
    requests.abort_all();
}

async fn send_ws(sender: &mut SplitSink<WebSocket, Message>, message: Message) -> bool {
    matches!(
        tokio::time::timeout(WS_SEND_TIMEOUT, sender.send(message)).await,
        Ok(Ok(()))
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_identity_survives_writes_but_changes_on_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("chat.db");
        std::fs::write(&database, "initial").unwrap();
        let initial = database_generation(database.to_str().unwrap()).unwrap();
        std::fs::write(&database, "ordinary message write").unwrap();
        assert_eq!(
            database_generation(database.to_str().unwrap()).unwrap(),
            initial
        );
        let replacement = dir.path().join("replacement.db");
        std::fs::write(&replacement, "new database").unwrap();
        std::fs::rename(replacement, &database).unwrap();
        assert_ne!(
            database_generation(database.to_str().unwrap()).unwrap(),
            initial
        );
    }
    use std::os::unix::fs::PermissionsExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};

    #[tokio::test]
    async fn websocket_forwards_event_while_rpc_request_is_slow() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json, sys, threading, time
lock = threading.Lock()
def reply(req):
    if req["method"] == "status":
        time.sleep(0.4)
    with lock:
        print(json.dumps({"jsonrpc":"2.0","id":req["id"],"result":{"ok":True}}), flush=True)
for line in sys.stdin:
    threading.Thread(target=reply, args=(json.loads(line),), daemon=True).start()
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        let (events, _) = broadcast::channel(16);
        let state = AppState {
            rpc,
            config: Config::default(),
            db_generation: Arc::new(RwLock::new("test".into())),
            database_path: String::new(),
            events: events.clone(),
            contacts: Arc::new(RwLock::new(ContactsState::Unavailable)),
            contacts_gate: Arc::new(Mutex::new(())),
        };
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (mut socket, _) = connect_async(format!("ws://{address}/ws")).await.unwrap();

        for _ in 0..2 {
            socket.next().await.unwrap().unwrap();
        }
        let request = Envelope::Req {
            id: "slow-request".into(),
            method: "status".into(),
            params: json!({}),
        };
        socket
            .send(ClientMessage::Text(request.to_line().unwrap().into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        events
            .send(Envelope::Event {
                topic: "message".into(),
                payload: json!({"guid": "event-before-response"}),
            })
            .unwrap();

        let first = tokio::time::timeout(Duration::from_millis(150), socket.next())
            .await
            .expect("event was blocked behind the RPC response")
            .unwrap()
            .unwrap();
        let ClientMessage::Text(line) = first else {
            panic!("expected text event");
        };
        assert_eq!(
            Envelope::parse_line(&line).unwrap(),
            Envelope::Event {
                topic: "message".into(),
                payload: json!({"guid": "event-before-response"}),
            }
        );
        server.abort();
    }
}
