use anyhow::{Context, Result};
use imsg_proto::{Envelope, ErrorBody};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, oneshot, Mutex};
use tracing::{debug, info, warn};

static REQ_ID: AtomicU64 = AtomicU64::new(1);

pub struct ImsgRpc {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    child: Arc<Mutex<Child>>,
    events: broadcast::Sender<Value>,
}

impl ImsgRpc {
    pub fn subscribe_events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub async fn spawn(imsg_path: &str) -> Result<Arc<Self>> {
        let (child, stdin, stdout) = launch_child(imsg_path).await?;
        let (events_tx, _) = broadcast::channel(256);
        let rpc = Arc::new(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            child: Arc::new(Mutex::new(child)),
            events: events_tx,
        });
        rpc.spawn_read_loop(stdout);
        Ok(rpc)
    }

    fn spawn_read_loop(self: &Arc<Self>, stdout: ChildStdout) {
        let reader = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = reader.read_loop(stdout).await {
                warn!("imsg rpc read loop ended: {e}");
            }
        });
    }

    /// Kills and relaunches the child, then re-establishes the watch
    /// subscription. Needed when a Contacts grant lands after the child
    /// started and the child's CNContactStore was created while denied.
    pub async fn respawn(self: &Arc<Self>, imsg_path: &str) -> Result<()> {
        {
            let mut child = self.child.lock().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let stale = {
            let mut pending = self.pending.lock().await;
            pending.drain().collect::<Vec<_>>()
        };
        for (_, tx) in stale {
            let _ = tx.send(json!({"error": {"message": "imsg rpc respawned"}}));
        }
        let (child, stdin, stdout) = launch_child(imsg_path).await?;
        *self.stdin.lock().await = stdin;
        *self.child.lock().await = child;
        self.spawn_read_loop(stdout);
        self.ensure_watch().await
    }

    /// Extracted from `spawn_watch_forwarder` so `respawn` reuses the retry loop.
    pub async fn ensure_watch(&self) -> Result<()> {
        let mut last_err = None;
        for _ in 0..6 {
            match self
                .call("watch.subscribe", json!({"debounce_ms": 500}))
                .await
            {
                Ok(_) => {
                    info!("watch subscription active");
                    return Ok(());
                }
                Err(e) => {
                    warn!("watch.subscribe failed: {e}");
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("watch.subscribe failed")))
    }

    async fn read_loop(self: Arc<Self>, stdout: ChildStdout) -> Result<()> {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).context("parse imsg line")?;
            if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                let mut pending = self.pending.lock().await;
                if let Some(tx) = pending.remove(id) {
                    let _ = tx.send(value);
                }
            } else if value.get("method").and_then(|m| m.as_str()) == Some("message") {
                if let Some(params) = value.get("params") {
                    if let Some(msg) = params.get("message") {
                        let _ = self.events.send(msg.clone());
                    }
                }
            } else if value.get("method").is_some() {
                debug!("imsg notification: {}", line);
            }
        }
        Ok(())
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = REQ_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("{req}\n").as_bytes())
            .await
            .context("write imsg rpc")?;
        stdin.flush().await?;

        let resp = rx.await.context("imsg rpc response channel")?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("imsg rpc error: {err}");
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn status(&self) -> Result<Value> {
        self.call("status", json!({})).await
    }
}

async fn launch_child(imsg_path: &str) -> Result<(Child, ChildStdin, ChildStdout)> {
    let mut child = Command::new(imsg_path)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn imsg rpc")?;
    let stdin = child.stdin.take().context("imsg stdin")?;
    let stdout = child.stdout.take().context("imsg stdout")?;
    Ok((child, stdin, stdout))
}

pub fn bridge_method_to_imsg(method: &str) -> Option<&'static str> {
    match method {
        "status" => Some("status"),
        "chats.list" => Some("chats.list"),
        "messages.history" => Some("messages.history"),
        "messages.after" => Some("messages.after"),
        "messages.search" => Some("messages.search"),
        "handles.check" => Some("handles.check"),
        "watch.ack" => None,
        "attachments.fetch" => None,
        _ => None,
    }
}

pub fn envelope_error(id: &str, code: &str, message: &str) -> Envelope {
    Envelope::Res {
        id: id.to_string(),
        ok: false,
        result: None,
        error: Some(ErrorBody {
            code: code.to_string(),
            message: message.to_string(),
        }),
    }
}

pub fn envelope_ok(id: &str, result: Value) -> Envelope {
    Envelope::Res {
        id: id.to_string(),
        ok: true,
        result: Some(result),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_bridge_methods() {
        assert_eq!(bridge_method_to_imsg("chats.list"), Some("chats.list"));
        assert_eq!(bridge_method_to_imsg("send"), None);
    }
}
