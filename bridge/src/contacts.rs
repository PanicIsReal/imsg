use crate::config::Config;
use crate::imsg_rpc::ImsgRpc;
use anyhow::{Context, Result};
use imsg_proto::ContactsState;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{info, warn};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const VERIFY_ATTEMPTS: u8 = 5;
const VERIFY_GAP: Duration = Duration::from_millis(400);
const AWAIT_POLL: Duration = Duration::from_millis(750);

/// TCC status as macOS reports it. Reading this never raises a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactsStatus {
    NotDetermined,
    Denied,
    Restricted,
    Authorized,
}

impl ContactsStatus {
    /// Granted on the wire only when names actually flow.
    pub fn as_wire(self, names_visible: bool) -> ContactsState {
        match self {
            Self::Authorized if names_visible => ContactsState::Granted,
            _ => ContactsState::Unavailable,
        }
    }

    fn from_probe_label(label: &str) -> Self {
        match label {
            "authorized" | "limited" => Self::Authorized,
            "denied" => Self::Denied,
            "restricted" => Self::Restricted,
            _ => Self::NotDetermined,
        }
    }
}

/// Terminal result of one authorize attempt.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ContactsOutcome {
    Granted { names_visible: bool },
    Denied,
    TimedOut,
    HelperMissing { detail: String },
}

impl ContactsOutcome {
    pub fn as_state(&self) -> ContactsState {
        match self {
            Self::Granted {
                names_visible: true,
            } => ContactsState::Granted,
            _ => ContactsState::Unavailable,
        }
    }
}

/// Reply when the single-in-flight gate is already held.
pub fn busy_gate_reply() -> Value {
    json!({"outcome": "prompting"})
}

/// Try to acquire the authorize gate without waiting. `None` means a dialog
/// is already up — callers must reply [`busy_gate_reply`] and not prompt again.
pub fn try_lock_gate(gate: &Mutex<()>) -> Option<tokio::sync::MutexGuard<'_, ()>> {
    gate.try_lock().ok()
}

/// Default install location of the status-only Swift probe.
pub fn probe_bin_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/libexec/imsg-contacts-probe")
}

/// True when any chat in a `chats.list` result has a non-empty `contact_name`.
/// The only honest test of whether names actually work.
pub fn chats_have_visible_names(result: &Value) -> bool {
    let Some(list) = result
        .get("chats")
        .and_then(|v| v.as_array())
        .or_else(|| result.as_array())
    else {
        return false;
    };
    if list.is_empty() {
        return false;
    }
    list.iter().any(|chat| {
        chat.get("contact_name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    })
}

/// First identifier or participant handle from a `chats.list` result.
pub fn first_handle(result: &Value) -> Option<String> {
    let list = result
        .get("chats")
        .and_then(|v| v.as_array())
        .or_else(|| result.as_array())?;
    for chat in list {
        if let Some(id) = nonempty_str(chat.get("identifier")) {
            return Some(id);
        }
        let Some(parts) = chat.get("participants").and_then(|v| v.as_array()) else {
            continue;
        };
        for part in parts {
            if let Some(s) = part.as_str().filter(|s| !s.trim().is_empty()) {
                return Some(s.to_string());
            }
            if let Some(id) =
                nonempty_str(part.get("identifier")).or_else(|| nonempty_str(part.get("handle")))
            {
                return Some(id);
            }
        }
    }
    None
}

fn nonempty_str(v: Option<&Value>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Reads TCC status. Never raises a dialog.
pub async fn probe(config: &Config) -> Result<ContactsStatus> {
    probe_with_handle(config, None).await
}

pub async fn probe_with_handle(config: &Config, handle: Option<&str>) -> Result<ContactsStatus> {
    match probe_via_swift().await {
        Ok(status) => Ok(status),
        Err(e) => {
            warn!("contacts probe binary: {e}; falling back to imsg nickname");
            probe_via_nickname(config, handle.unwrap_or("0")).await
        }
    }
}

async fn probe_via_swift() -> Result<ContactsStatus> {
    let bin = probe_bin_path();
    if !bin.is_file() {
        anyhow::bail!("missing {}", bin.display());
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, Command::new(&bin).output())
        .await
        .context("contacts-probe timed out")?
        .with_context(|| format!("spawn {}", bin.display()))?;
    parse_probe_output(&output.stdout, &output.stderr)
}

fn parse_probe_output(stdout: &[u8], stderr: &[u8]) -> Result<ContactsStatus> {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(line) {
        if let Some(status) = v.get("status").and_then(|s| s.as_str()) {
            return Ok(ContactsStatus::from_probe_label(status));
        }
    }
    let err = String::from_utf8_lossy(stderr);
    anyhow::bail!("contacts-probe unreadable: {line} {err}");
}

/// Subprocess with no TTY — `forStdin(isTTY:false)` will not raise a dialog.
async fn probe_via_nickname(config: &Config, handle: &str) -> Result<ContactsStatus> {
    let imsg = resolve_imsg_path(&config.imsg_path);
    let output = tokio::time::timeout(
        PROBE_TIMEOUT,
        Command::new(&imsg)
            .args(["nickname", "--address", handle, "--local", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("imsg nickname probe timed out")?
    .with_context(|| format!("spawn {} nickname", imsg.display()))?;
    Ok(status_from_nickname(
        &output.stdout,
        &output.stderr,
        output.status.success(),
    ))
}

fn status_from_nickname(stdout: &[u8], stderr: &[u8], success: bool) -> ContactsStatus {
    let combined = format!(
        "{} {}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lower = combined.to_ascii_lowercase();
    if lower.contains("denied") {
        return ContactsStatus::Denied;
    }
    if lower.contains("restricted") {
        return ContactsStatus::Restricted;
    }
    if lower.contains("contacts_unavailable") || lower.contains("not determined") {
        return ContactsStatus::NotDetermined;
    }
    if success {
        return ContactsStatus::Authorized;
    }
    ContactsStatus::NotDetermined
}

pub async fn names_visible(rpc: &ImsgRpc) -> Result<bool> {
    let result = rpc.call("chats.list", json!({"limit": 20})).await?;
    Ok(chats_have_visible_names(&result))
}

/// Raises the system Contacts dialog in a Ghostty-parented TTY and drives
/// the attempt to a terminal state.
pub async fn authorize(
    config: &Config,
    rpc: &Arc<ImsgRpc>,
    timeout: Duration,
) -> Result<ContactsOutcome> {
    match tokio::time::timeout(timeout, authorize_inner(config, rpc)).await {
        Ok(result) => result,
        Err(_) => Ok(ContactsOutcome::TimedOut),
    }
}

enum Step {
    Probe,
    Prompt,
    AwaitUser { deadline: Instant },
    VerifyNames { attempt: u8 },
    RestartRpc,
    Done(ContactsOutcome),
}

async fn authorize_inner(config: &Config, rpc: &Arc<ImsgRpc>) -> Result<ContactsOutcome> {
    let mut step = Step::Probe;
    let mut restarted = false;
    let await_deadline = Instant::now() + Duration::from_secs(90);
    loop {
        step = match step {
            Step::Probe => {
                let handle = pick_handle(rpc).await.ok();
                let status = probe_with_handle(config, handle.as_deref()).await?;
                match status {
                    ContactsStatus::Denied | ContactsStatus::Restricted => {
                        Step::Done(ContactsOutcome::Denied)
                    }
                    ContactsStatus::Authorized => Step::VerifyNames { attempt: 0 },
                    ContactsStatus::NotDetermined => Step::Prompt,
                }
            }
            Step::Prompt => match pick_handle(rpc).await {
                Ok(handle) => match prompt_via_ghostty(config, &handle).await {
                    Ok(()) => Step::AwaitUser {
                        deadline: await_deadline,
                    },
                    Err(outcome) => Step::Done(outcome),
                },
                Err(e) => Step::Done(ContactsOutcome::HelperMissing {
                    detail: e.to_string(),
                }),
            },
            Step::AwaitUser { deadline } => {
                if Instant::now() >= deadline {
                    Step::Done(ContactsOutcome::TimedOut)
                } else {
                    tokio::time::sleep(AWAIT_POLL).await;
                    let handle = pick_handle(rpc).await.ok();
                    match probe_with_handle(config, handle.as_deref()).await? {
                        ContactsStatus::Authorized => Step::VerifyNames { attempt: 0 },
                        ContactsStatus::Denied | ContactsStatus::Restricted => {
                            Step::Done(ContactsOutcome::Denied)
                        }
                        ContactsStatus::NotDetermined => Step::AwaitUser { deadline },
                    }
                }
            }
            Step::VerifyNames { attempt } => {
                if names_visible(rpc).await.unwrap_or(false) {
                    Step::Done(ContactsOutcome::Granted {
                        names_visible: true,
                    })
                } else if attempt + 1 < VERIFY_ATTEMPTS {
                    tokio::time::sleep(VERIFY_GAP).await;
                    Step::VerifyNames {
                        attempt: attempt + 1,
                    }
                } else if !restarted {
                    Step::RestartRpc
                } else {
                    let handle = pick_handle(rpc).await.ok();
                    match probe_with_handle(config, handle.as_deref()).await? {
                        ContactsStatus::Authorized => Step::Done(ContactsOutcome::Granted {
                            names_visible: false,
                        }),
                        ContactsStatus::Denied | ContactsStatus::Restricted => {
                            Step::Done(ContactsOutcome::Denied)
                        }
                        ContactsStatus::NotDetermined => Step::Done(ContactsOutcome::TimedOut),
                    }
                }
            }
            Step::RestartRpc => {
                info!("contacts grant landed without names; respawning imsg rpc");
                restarted = true;
                if let Err(e) = rpc.respawn(&config.imsg_path).await {
                    warn!("imsg rpc respawn: {e}");
                    Step::Done(ContactsOutcome::HelperMissing {
                        detail: format!("respawn failed: {e}"),
                    })
                } else {
                    Step::VerifyNames { attempt: 0 }
                }
            }
            Step::Done(outcome) => return Ok(outcome),
        };
    }
}

async fn pick_handle(rpc: &ImsgRpc) -> Result<String> {
    let result = rpc.call("chats.list", json!({"limit": 20})).await?;
    first_handle(&result).context("no chat handle available to request Contacts")
}

/// `open -na Ghostty.app --args -e "<imsg> nickname --local"` — a TTY stdin, so
/// `ContactsAccessPolicy::forStdin(isTTY: true)` prompts instead of skipping.
async fn prompt_via_ghostty(config: &Config, handle: &str) -> Result<(), ContactsOutcome> {
    let ghostty = PathBuf::from(&config.ghostty_path);
    if !ghostty.exists() {
        return Err(ContactsOutcome::HelperMissing {
            detail: format!("Ghostty not found at {}", ghostty.display()),
        });
    }
    let imsg = resolve_imsg_path(&config.imsg_path);
    if !imsg_usable(&imsg) {
        return Err(ContactsOutcome::HelperMissing {
            detail: format!("imsg not found at {}", imsg.display()),
        });
    }
    let status = Command::new("open")
        .arg("-na")
        .arg(&ghostty)
        .arg("--args")
        .arg("-e")
        .arg(&imsg)
        .arg("nickname")
        .arg("--address")
        .arg(handle)
        .arg("--local")
        .arg("--json")
        .status()
        .await
        .map_err(|e| ContactsOutcome::HelperMissing {
            detail: format!("open Ghostty: {e}"),
        })?;
    if !status.success() {
        return Err(ContactsOutcome::HelperMissing {
            detail: format!("open Ghostty exited {status}"),
        });
    }
    Ok(())
}

fn imsg_usable(path: &Path) -> bool {
    path.is_file() || path.components().count() == 1
}

pub fn resolve_imsg_path(imsg_path: &str) -> PathBuf {
    if let Some(found) = crate::steipete::resolve_steipete_imsg(imsg_path) {
        return found;
    }
    let p = PathBuf::from(imsg_path);
    if p.is_absolute() {
        return p;
    }
    if let Some(found) = search_path(imsg_path) {
        return found;
    }
    let brew = PathBuf::from("/opt/homebrew/bin").join(imsg_path);
    if brew.exists() {
        return brew;
    }
    p
}

fn search_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_visible_requires_nonempty_contact_name() {
        let empty = json!({"chats": []});
        assert!(!chats_have_visible_names(&empty));

        let handles_only = json!({
            "chats": [
                {"id": 1, "identifier": "+15551212", "contact_name": ""},
                {"id": 2, "name": "Ada"}
            ]
        });
        assert!(!chats_have_visible_names(&handles_only));

        let named = json!({
            "chats": [
                {"id": 1, "identifier": "+15551212", "contact_name": ""},
                {"id": 2, "contact_name": "Jamie Chen"}
            ]
        });
        assert!(chats_have_visible_names(&named));
    }

    #[test]
    fn first_handle_prefers_identifier_then_participants() {
        let chats = json!({
            "chats": [{
                "id": 1,
                "identifier": "+15550134",
                "participants": ["+15550999"]
            }]
        });
        assert_eq!(first_handle(&chats).as_deref(), Some("+15550134"));

        let participants_only = json!({
            "chats": [{
                "id": 2,
                "participants": [{"identifier": "+15550999"}]
            }]
        });
        assert_eq!(
            first_handle(&participants_only).as_deref(),
            Some("+15550999")
        );
    }

    #[test]
    fn authorized_without_names_is_unavailable_on_the_wire() {
        assert_eq!(
            ContactsStatus::Authorized.as_wire(true),
            ContactsState::Granted
        );
        assert_eq!(
            ContactsStatus::Authorized.as_wire(false),
            ContactsState::Unavailable
        );
        assert_eq!(
            ContactsOutcome::Granted {
                names_visible: false
            }
            .as_state(),
            ContactsState::Unavailable
        );
    }

    #[tokio::test]
    async fn second_authorize_while_locked_returns_prompting() {
        let gate = Mutex::new(());
        let _hold = gate.lock().await;
        assert!(
            try_lock_gate(&gate).is_none(),
            "held gate must reject a second lock"
        );
        assert_eq!(busy_gate_reply()["outcome"], "prompting");
    }

    #[test]
    fn nickname_maps_contacts_unavailable_without_treating_it_as_granted() {
        let status = status_from_nickname(br#"{"error":"contacts_unavailable"}"#, b"", false);
        assert_eq!(status, ContactsStatus::NotDetermined);
        let ok = status_from_nickname(br#"{"nickname":"Jamie"}"#, b"", true);
        assert_eq!(ok, ContactsStatus::Authorized);
    }
}
