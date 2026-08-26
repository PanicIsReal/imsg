use crate::config::SyncConfig;
use anyhow::{Context, Result};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use imsg_proto::event::BridgeEvent;
use imsg_proto::{Envelope, ErrorBody};
use rustls::{ClientConfig, RootCertStore};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream,
};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = SplitSink<WsStream, Message>;
type WsRead = SplitStream<WsStream>;

static REQ_ID: AtomicU64 = AtomicU64::new(1);

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Owns the WSS to the Mac. Demultiplexes inbound frames: `res` routed by id to
/// the waiting caller, `event` routed to the session's event channel.
pub struct Uplink {
    write: Mutex<WsSink>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, ErrorBody>>>>,
    events: mpsc::Sender<BridgeEvent>,
}

/// What `connect` hands back: the call surface, the event stream, and the pump
/// task whose completion *is* the disconnect signal.
pub struct UplinkSession {
    pub uplink: Arc<Uplink>,
    pub events: mpsc::Receiver<BridgeEvent>,
    pub pump: JoinHandle<Result<()>>,
}

#[derive(Clone, Default)]
pub struct UplinkHandle(Arc<RwLock<Option<Arc<Uplink>>>>);

#[derive(Debug, thiserror::Error)]
pub enum UplinkError {
    #[error("mac link is down")]
    LinkDown,
    #[error("timed out awaiting {method}")]
    Timeout { method: String },
    #[error("{0:?}")]
    Upstream(ErrorBody),
    #[error(transparent)]
    Transport(#[from] anyhow::Error),
}

impl UplinkError {
    /// Stable code for the socket `res.error.code` the plugin surfaces.
    pub fn code(&self) -> &'static str {
        match self {
            Self::LinkDown => "link_down",
            Self::Timeout { .. } => "timeout",
            Self::Upstream(_) => "upstream",
            Self::Transport(_) => "error",
        }
    }
}

/// Where one inbound envelope goes. Extracted so tests can prove an Event
/// arriving while a call is pending is forwarded and the matching Res still resolves.
#[derive(Debug)]
pub(crate) enum RoutedFrame {
    Response {
        id: String,
        result: Result<Value, ErrorBody>,
    },
    Event(BridgeEvent),
    Ping,
    Ignore,
}

pub(crate) fn route_frame(env: Envelope) -> RoutedFrame {
    match env {
        Envelope::Res {
            id,
            ok,
            result,
            error,
        } => {
            let result = if ok {
                Ok(result.unwrap_or(Value::Null))
            } else {
                Err(error.unwrap_or_else(|| ErrorBody {
                    code: "error".into(),
                    message: "upstream error".into(),
                }))
            };
            RoutedFrame::Response { id, result }
        }
        env @ Envelope::Event { .. } => match BridgeEvent::from_envelope(&env) {
            Some(evt) => RoutedFrame::Event(evt),
            None => RoutedFrame::Ignore,
        },
        Envelope::Ping => RoutedFrame::Ping,
        Envelope::Pong | Envelope::Req { .. } => RoutedFrame::Ignore,
    }
}

/// Applies one routed frame to the pending map / event sink. Sync so tests do
/// not need a live Mac or the WSS pump.
#[cfg(test)]
pub(crate) fn deliver_frame(
    env: Envelope,
    pending: &mut HashMap<String, oneshot::Sender<Result<Value, ErrorBody>>>,
    events: &mpsc::Sender<BridgeEvent>,
) {
    match route_frame(env) {
        RoutedFrame::Response { id, result } => {
            if let Some(tx) = pending.remove(&id) {
                let _ = tx.send(result);
            }
        }
        RoutedFrame::Event(evt) => {
            let _ = events.try_send(evt);
        }
        RoutedFrame::Ping | RoutedFrame::Ignore => {}
    }
}

impl Uplink {
    pub async fn connect(config: &SyncConfig) -> Result<UplinkSession> {
        let tls = build_tls(config)?;
        let connector = Connector::Rustls(Arc::new(tls));
        let (ws, _) =
            connect_async_tls_with_config(&config.bridge_url, None, false, Some(connector))
                .await
                .context("connect bridge")?;
        let (write, read) = ws.split();
        let (evt_tx, evt_rx) = mpsc::channel(256);
        let uplink = Arc::new(Self {
            write: Mutex::new(write),
            pending: Mutex::new(HashMap::new()),
            events: evt_tx,
        });
        let pump_uplink = Arc::clone(&uplink);
        let pump = tokio::spawn(async move { pump_uplink.pump(read).await });
        Ok(UplinkSession {
            uplink,
            events: evt_rx,
            pump,
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, UplinkError> {
        self.call_timeout(method, params, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, UplinkError> {
        let id = REQ_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);

        let req = Envelope::Req {
            id: id.clone(),
            method: method.into(),
            params,
        };
        let line = req.to_line().map_err(anyhow::Error::from)?;
        let send = {
            let mut write = self.write.lock().await;
            write.send(Message::Text(line.into())).await
        };
        if let Err(e) = send {
            self.pending.lock().await.remove(&id);
            return Err(UplinkError::Transport(e.into()));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(body))) => Err(UplinkError::Upstream(body)),
            Ok(Err(_)) => Err(UplinkError::Transport(anyhow::anyhow!(
                "response channel closed"
            ))),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(UplinkError::Timeout {
                    method: method.into(),
                })
            }
        }
    }

    async fn pump(self: Arc<Self>, mut read: WsRead) -> Result<()> {
        let result = self.read_loop(&mut read).await;
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(ErrorBody {
                code: "link_down".into(),
                message: "connection closed".into(),
            }));
        }
        result
    }

    async fn read_loop(&self, read: &mut WsRead) -> Result<()> {
        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(text) => {
                    let Ok(env) = Envelope::parse_line(&text) else {
                        continue;
                    };
                    match route_frame(env) {
                        RoutedFrame::Response { id, result } => {
                            if let Some(tx) = self.pending.lock().await.remove(&id) {
                                let _ = tx.send(result);
                            }
                        }
                        RoutedFrame::Event(evt) => {
                            if self.events.send(evt).await.is_err() {
                                break;
                            }
                        }
                        RoutedFrame::Ping => {
                            let line = Envelope::Pong.to_line()?;
                            let mut write = self.write.lock().await;
                            write.send(Message::Text(line.into())).await?;
                        }
                        RoutedFrame::Ignore => {}
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok(())
    }
}

impl UplinkHandle {
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, UplinkError> {
        self.call_timeout(method, params, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, UplinkError> {
        let uplink = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        uplink.call_timeout(method, params, timeout).await
    }

    pub(crate) async fn attach(&self, uplink: Arc<Uplink>) {
        *self.0.write().await = Some(uplink);
    }

    pub(crate) async fn detach(&self) {
        *self.0.write().await = None;
    }
}

fn build_tls(config: &SyncConfig) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    let ca = std::fs::read(&config.ca_cert_path).context("read ca")?;
    for cert in rustls_pemfile::certs(&mut ca.as_slice()) {
        roots.add(cert?).context("add ca")?;
    }
    let cert_pem = std::fs::read(&config.client_cert_path).context("read client cert")?;
    let key_pem = std::fs::read(&config.client_key_path).context("read client key")?;
    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())?
        .ok_or_else(|| anyhow::anyhow!("no client key"))?;

    ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)
        .context("client tls")
}

#[cfg(test)]
mod tests {
    use super::*;
    use imsg_proto::event::BridgeEvent;
    use serde_json::json;

    #[tokio::test]
    async fn event_arriving_while_call_pending_is_forwarded_and_matching_res_still_resolves() {
        let mut pending = HashMap::new();
        let (evt_tx, mut evt_rx) = mpsc::channel(8);
        let (resp_tx, resp_rx) = oneshot::channel();
        pending.insert("42".into(), resp_tx);

        deliver_frame(
            Envelope::Event {
                topic: "message".into(),
                payload: json!({"id": 9, "chat_id": 1, "text": "hello"}),
            },
            &mut pending,
            &evt_tx,
        );

        let evt = evt_rx.try_recv().expect("event must reach the channel");
        match evt {
            BridgeEvent::Message(v) => {
                assert_eq!(v["id"], 9);
                assert_eq!(v["text"], "hello");
            }
            other => panic!("expected message event, got {other:?}"),
        }
        assert!(
            pending.contains_key("42"),
            "event must not complete the pending call"
        );

        deliver_frame(
            Envelope::Res {
                id: "42".into(),
                ok: true,
                result: Some(json!({"ok": true})),
                error: None,
            },
            &mut pending,
            &evt_tx,
        );

        let result = resp_rx.await.expect("oneshot still live").expect("ok res");
        assert_eq!(result["ok"], true);
        assert!(pending.is_empty());
        assert!(
            evt_rx.try_recv().is_err(),
            "res must not appear as an event"
        );
    }

    #[tokio::test]
    async fn detached_handle_fails_fast_with_link_down() {
        let handle = UplinkHandle::default();
        let err = handle.call("send", json!({})).await.unwrap_err();
        assert!(matches!(err, UplinkError::LinkDown));
        assert_eq!(err.code(), "link_down");
    }

    #[test]
    fn route_frame_classifies_res_error_and_unknown_event() {
        match route_frame(Envelope::Res {
            id: "7".into(),
            ok: false,
            result: None,
            error: Some(ErrorBody {
                code: "denied".into(),
                message: "no".into(),
            }),
        }) {
            RoutedFrame::Response { id, result } => {
                assert_eq!(id, "7");
                let err = result.expect_err("error res");
                assert_eq!(err.code, "denied");
            }
            other => panic!("expected response, got {other:?}"),
        }

        match route_frame(Envelope::Event {
            topic: "future.thing".into(),
            payload: json!({}),
        }) {
            RoutedFrame::Event(BridgeEvent::Unknown { topic }) => {
                assert_eq!(topic, "future.thing");
            }
            other => panic!("expected unknown event, got {other:?}"),
        }
    }
}
