use anyhow::{Context, Result};
use imsg_proto::{Envelope, ErrorBody};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::time::Instant as TokioInstant;
use tracing::{debug, info, warn};

static REQ_ID: AtomicU64 = AtomicU64::new(1);
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
const WATCH_DEBOUNCE_MS: u64 = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum RpcEvent {
    Message(Value),
    Gap { reason: &'static str },
}

type Pending = Arc<StdMutex<HashMap<String, oneshot::Sender<Value>>>>;

struct PendingGuard {
    id: String,
    pending: Pending,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

pub struct ImsgRpc {
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    child: Mutex<Option<Child>>,
    lifecycle: Mutex<()>,
    watch_generation: Mutex<Option<u64>>,
    watch_subscription: AtomicU64,
    generation: AtomicU64,
    imsg_path: String,
    events: broadcast::Sender<RpcEvent>,
}

impl ImsgRpc {
    pub fn subscribe_events(&self) -> broadcast::Receiver<RpcEvent> {
        self.events.subscribe()
    }

    pub async fn spawn(imsg_path: &str) -> Result<Arc<Self>> {
        let (child, stdin, stdout) = launch_child(imsg_path).await?;
        let (events_tx, _) = broadcast::channel(256);
        let rpc = Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            pending: Arc::new(StdMutex::new(HashMap::new())),
            child: Mutex::new(Some(child)),
            lifecycle: Mutex::new(()),
            watch_generation: Mutex::new(None),
            watch_subscription: AtomicU64::new(0),
            generation: AtomicU64::new(1),
            imsg_path: imsg_path.to_string(),
            events: events_tx,
        });
        rpc.spawn_read_loop(stdout, 1);
        Ok(rpc)
    }

    fn spawn_read_loop(self: &Arc<Self>, stdout: ChildStdout, generation: u64) {
        let reader = Arc::clone(self);
        tokio::spawn(async move {
            let result = Arc::clone(&reader).read_loop(stdout).await;
            if reader.generation.load(Ordering::Acquire) != generation {
                return;
            }
            match result {
                Ok(()) => warn!(generation, "imsg rpc reached EOF"),
                Err(ref error) => warn!(generation, %error, "imsg rpc read loop ended"),
            }
            while reader.generation.load(Ordering::Acquire) == generation {
                match reader.restart_if_current(generation).await {
                    Ok(()) => break,
                    Err(error) => {
                        warn!(generation, %error, "imsg rpc automatic restart failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    pub async fn respawn(self: &Arc<Self>) -> Result<()> {
        self.restart(None).await
    }

    async fn restart_if_current(self: &Arc<Self>, generation: u64) -> Result<()> {
        self.restart(Some(generation)).await
    }

    async fn restart(self: &Arc<Self>, expected_generation: Option<u64>) -> Result<()> {
        let lifecycle = self.lifecycle.lock().await;
        if expected_generation
            .is_some_and(|expected| self.generation.load(Ordering::Acquire) != expected)
        {
            return Ok(());
        }

        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let mut stdin = tokio::time::timeout(RPC_TIMEOUT, self.stdin.lock())
            .await
            .context("timed out waiting for imsg stdin during restart")?;
        *stdin = None;
        drop(stdin);
        self.fail_pending("imsg rpc restarted");

        let (child, stdin, stdout) = launch_child(&self.imsg_path).await?;
        *self.stdin.lock().await = Some(stdin);
        *self.child.lock().await = Some(child);
        self.watch_subscription.store(0, Ordering::Release);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.spawn_read_loop(stdout, generation);
        let _ = self.events.send(RpcEvent::Gap {
            reason: "rpc_restart",
        });
        info!(generation, "imsg rpc restarted");

        drop(lifecycle);
        Ok(())
    }

    fn fail_pending(&self, message: &str) {
        let stale = self
            .pending
            .lock()
            .unwrap()
            .drain()
            .map(|(_, sender)| sender)
            .collect::<Vec<_>>();
        for sender in stale {
            let _ = sender.send(json!({"error": {"message": message}}));
        }
    }

    pub async fn ensure_watch(&self) -> Result<()> {
        let mut subscribed_generation = self.watch_generation.lock().await;
        let mut last_err = None;
        for _ in 0..6 {
            let generation = self.generation.load(Ordering::Acquire);
            if *subscribed_generation == Some(generation)
                && self.watch_subscription.load(Ordering::Acquire) != 0
            {
                return Ok(());
            }
            match self.subscribe_watch().await {
                Ok(()) => {
                    if self.generation.load(Ordering::Acquire) != generation {
                        continue;
                    }
                    *subscribed_generation = Some(generation);
                    info!(debounce_ms = WATCH_DEBOUNCE_MS, "watch subscription active");
                    return Ok(());
                }
                Err(error) => {
                    warn!(%error, "watch.subscribe failed");
                    last_err = Some(error);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("watch.subscribe failed")))
    }

    async fn subscribe_watch(&self) -> Result<()> {
        self.call("watch.subscribe", json!({"debounce_ms": WATCH_DEBOUNCE_MS}))
            .await
            .map(|_| ())
    }

    async fn read_loop(self: Arc<Self>, stdout: ChildStdout) -> Result<()> {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).context("parse imsg line")?;
            if let Some(id) = value.get("id").and_then(Value::as_str) {
                if let Some(subscription) = value
                    .pointer("/result/subscription")
                    .and_then(Value::as_u64)
                {
                    self.watch_subscription
                        .store(subscription, Ordering::Release);
                }
                if let Some(sender) = self.pending.lock().unwrap().remove(id) {
                    let _ = sender.send(value);
                }
            } else if value.get("method").and_then(Value::as_str) == Some("message") {
                if let Some(message) = value.pointer("/params/message") {
                    let _ = self.events.send(RpcEvent::Message(message.clone()));
                }
            } else if value.get("method").and_then(Value::as_str) == Some("watch.overflow") {
                let subscription = value
                    .pointer("/params/subscription")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let current = self.watch_subscription.load(Ordering::Acquire);
                if current != 0 && subscription != 0 && subscription != current {
                    debug!(subscription, current, "ignored stale watch.overflow");
                    continue;
                }
                self.watch_subscription.store(0, Ordering::Release);
                let _ = self.events.send(RpcEvent::Gap {
                    reason: "watch_overflow",
                });
            } else if value.get("method").is_some() {
                debug!(method = ?value.get("method"), "imsg notification");
            }
        }
        Ok(())
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_with_timeout(method, params, RPC_TIMEOUT).await
    }

    async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = REQ_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let started = Instant::now();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), sender);
        let _pending = PendingGuard {
            id: id.clone(),
            pending: Arc::clone(&self.pending),
        };

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let deadline = TokioInstant::now() + timeout;
        let transaction = async {
            {
                let mut stdin = self.stdin.lock().await;
                let stdin = stdin
                    .as_mut()
                    .context("imsg rpc unavailable while restarting")?;
                stdin
                    .write_all(format!("{request}\n").as_bytes())
                    .await
                    .context("write imsg rpc")?;
                stdin.flush().await.context("flush imsg rpc")?;
            }
            receiver.await.context("imsg rpc response channel")
        };
        let response = tokio::time::timeout_at(deadline, transaction)
            .await
            .with_context(|| format!("imsg rpc timeout after {}ms", timeout.as_millis()))
            .and_then(|response| response);
        debug!(
            metric = "rpc_roundtrip",
            elapsed_ms = started.elapsed().as_millis() as u64,
            request_id = %id,
            rpc_method = method,
            "realtime latency"
        );
        let response = response?;
        if let Some(error) = response.get("error") {
            anyhow::bail!("imsg rpc error: {error}");
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn status(&self) -> Result<Value> {
        self.call("status", json!({})).await
    }
}

async fn launch_child(imsg_path: &str) -> Result<(Child, ChildStdin, ChildStdout)> {
    let imsg_path = crate::steipete::ensure_steipete_imsg(imsg_path)?;
    let mut child = Command::new(&imsg_path)
        .arg("rpc")
        .kill_on_drop(true)
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
        "send" => Some("send"),
        "watch.ack" | "attachments.fetch" => None,
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
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn maps_bridge_methods() {
        assert_eq!(bridge_method_to_imsg("chats.list"), Some("chats.list"));
        assert_eq!(bridge_method_to_imsg("send"), Some("send"));
        assert_eq!(bridge_method_to_imsg("watch.ack"), None);
    }

    #[test]
    fn dropping_pending_call_removes_its_entry() {
        let pending = Arc::new(StdMutex::new(HashMap::new()));
        let (sender, _receiver) = oneshot::channel();
        pending.lock().unwrap().insert("42".into(), sender);
        {
            let _guard = PendingGuard {
                id: "42".into(),
                pending: Arc::clone(&pending),
            };
        }
        assert!(pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn slow_response_does_not_hold_the_stdin_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json, sys, threading, time
lock = threading.Lock()
def reply(req):
    if req["method"] == "slow":
        time.sleep(0.3)
    out = json.dumps({"jsonrpc":"2.0","id":req["id"],"result":{"method":req["method"]}})
    with lock:
        print(out, flush=True)
for line in sys.stdin:
    threading.Thread(target=reply, args=(json.loads(line),), daemon=True).start()
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        assert_eq!(
            rpc.call("warmup", json!({})).await.unwrap(),
            json!({"method": "warmup"})
        );
        let slow_rpc = Arc::clone(&rpc);
        let slow = tokio::spawn(async move { slow_rpc.call("slow", json!({})).await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        let fast = tokio::time::timeout(Duration::from_millis(150), rpc.call("fast", json!({})))
            .await
            .expect("fast request was blocked behind the slow response")
            .unwrap();
        assert_eq!(fast, json!({"method": "fast"}));
        assert_eq!(slow.await.unwrap().unwrap(), json!({"method": "slow"}));
    }

    #[tokio::test]
    async fn eof_restarts_child_and_signals_a_gap() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        let count_path = temp.path().join("starts");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
count_path = pathlib.Path({count_path:?})
count = int(count_path.read_text()) + 1 if count_path.exists() else 1
count_path.write_text(str(count))
for line in sys.stdin:
    if count == 1:
        sys.exit(0)
    req = json.loads(line)
    print(json.dumps({{"jsonrpc":"2.0","id":req["id"],"result":{{"generation":count}}}}), flush=True)
"#,
            count_path = count_path.to_string_lossy()
        );
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        let mut events = rpc.subscribe_events();
        assert!(rpc.call("exit", json!({})).await.is_err());
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("restart did not signal a gap")
            .unwrap();
        assert_eq!(
            event,
            RpcEvent::Gap {
                reason: "rpc_restart"
            }
        );
        assert_eq!(
            rpc.call("status", json!({})).await.unwrap(),
            json!({"generation": 2})
        );
    }

    #[tokio::test]
    async fn timeout_removes_pending_request() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        std::fs::write(
            &path,
            "#!/usr/bin/env python3\nimport time\ntime.sleep(10)\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        let error = rpc
            .call_with_timeout(
                "hang",
                json!({"payload": "x".repeat(4 * 1024 * 1024)}),
                Duration::from_millis(25),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timeout after 25ms"));
        assert!(rpc.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn watch_subscription_runs_once_per_child_generation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        let calls_path = temp.path().join("watch-calls");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
calls_path = pathlib.Path({calls_path:?})
for line in sys.stdin:
    req = json.loads(line)
    if req["method"] == "watch.subscribe":
        with calls_path.open("a") as calls:
            calls.write("watch\n")
    print(json.dumps({{"jsonrpc":"2.0","id":req["id"],"result":{{"subscription":1}}}}), flush=True)
"#,
            calls_path = calls_path.to_string_lossy()
        );
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        rpc.ensure_watch().await.unwrap();
        rpc.ensure_watch().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&calls_path)
                .unwrap()
                .lines()
                .count(),
            1
        );

        rpc.respawn().await.unwrap();
        rpc.ensure_watch().await.unwrap();
        rpc.ensure_watch().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&calls_path)
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn watch_overflow_invalidates_the_subscription_and_signals_a_gap() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake-imsg");
        let calls_path = temp.path().join("watch-calls");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
calls_path = pathlib.Path({calls_path:?})
subs = 0
for line in sys.stdin:
    req = json.loads(line)
    if req["method"] != "watch.subscribe":
        print(json.dumps({{"jsonrpc":"2.0","id":req["id"],"result":{{"ok":True}}}}), flush=True)
        continue
    subs += 1
    with calls_path.open("a") as calls:
        calls.write("watch\n")
    print(json.dumps({{"jsonrpc":"2.0","id":req["id"],"result":{{"subscription":subs}}}}), flush=True)
    print(json.dumps({{"jsonrpc":"2.0","method":"message","params":{{"subscription":subs,"message":{{"id":subs}}}}}}), flush=True)
    if subs == 1:
        print(json.dumps({{"jsonrpc":"2.0","method":"watch.overflow","params":{{"subscription":1,"resume_after_rowid":1,"reason":"buffer_limit_exceeded","terminal":True}}}}), flush=True)
"#,
            calls_path = calls_path.to_string_lossy()
        );
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();

        let rpc = ImsgRpc::spawn(path.to_str().unwrap()).await.unwrap();
        let mut events = rpc.subscribe_events();
        rpc.ensure_watch().await.unwrap();

        let first = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("first watch message")
            .unwrap();
        assert_eq!(first, RpcEvent::Message(json!({"id": 1})));
        let gap = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("overflow did not signal a gap")
            .unwrap();
        assert_eq!(
            gap,
            RpcEvent::Gap {
                reason: "watch_overflow"
            }
        );
        assert_eq!(
            std::fs::read_to_string(&calls_path)
                .unwrap()
                .lines()
                .count(),
            1
        );

        rpc.ensure_watch().await.unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("resubscribe did not emit a later message")
            .unwrap();
        assert_eq!(second, RpcEvent::Message(json!({"id": 2})));
        rpc.ensure_watch().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&calls_path)
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
