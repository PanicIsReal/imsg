pub(crate) mod secret;
pub(crate) mod store;

use crate::cache::MessageCache;
use crate::config::ServerUrl;
use crate::uplink::UplinkHandle;
use anyhow::{bail, Context, Result};
use imsg_proto::Envelope;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, watch, RwLock};
use url::Url;

#[derive(Debug, Clone, Serialize)]
pub struct PublicView {
    pub server_url: Option<String>,
    pub password_set: bool,
    pub session: SessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Unconfigured,
    Connecting,
    Live,
    Down,
}

impl PublicView {
    pub fn to_status_fields(&self) -> Value {
        json!({
            "server_url": self.server_url,
            "password_set": self.password_set,
            "session": self.session,
        })
    }
}

#[derive(Clone)]
pub struct Password(String);

impl Password {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let s = raw.into();
        if s.trim().is_empty() {
            bail!("password is empty");
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

pub enum PasswordUpdate {
    Keep,
    Set(Password),
}

pub struct SettingsDraft {
    pub(crate) server: ServerUrl,
    pub(crate) password: PasswordUpdate,
}

impl SettingsDraft {
    pub fn from_input(url: &str, password: Option<&str>) -> Result<Self> {
        let server = coerce_server_url(url)?;
        let password = match password.map(str::trim).filter(|s| !s.is_empty()) {
            None => PasswordUpdate::Keep,
            Some(raw) => PasswordUpdate::Set(Password::new(raw)?),
        };
        Ok(Self { server, password })
    }
}

#[derive(Clone, Debug)]
pub struct PublicConfig {
    pub server: ServerUrl,
    pub cache_path: PathBuf,
    pub socket_path: PathBuf,
    pub prefetch_chats: u32,
    pub prefetch_messages: u32,
}

#[derive(Clone)]
pub struct Credentials {
    pub public: PublicConfig,
    secret: Password,
}

impl Credentials {
    pub(crate) fn new(public: PublicConfig, secret: Password) -> Self {
        Self { public, secret }
    }

    pub fn password(&self) -> &Password {
        &self.secret
    }
}

#[derive(Clone)]
pub enum Provision {
    Empty,
    Ready(Credentials),
}

pub struct Link {
    ctx: store::StoreCtx,
    cache_path: PathBuf,
    socket_path: PathBuf,
    provision: RwLock<Provision>,
    uplink: UplinkHandle,
    generation: AtomicU64,
    wake: watch::Sender<u64>,
    _wake_hold: watch::Receiver<u64>,
    connecting: AtomicBool,
}

impl Link {
    pub fn boot() -> Result<Arc<Self>> {
        Self::boot_with(store::StoreCtx::production())
    }

    pub(crate) fn boot_with(ctx: store::StoreCtx) -> Result<Arc<Self>> {
        let provision = match store::load(&ctx) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("store load failed, starting unconfigured: {e}");
                Provision::Empty
            }
        };
        let (cache_path, socket_path) = match &provision {
            Provision::Ready(creds) => (
                creds.public.cache_path.clone(),
                creds.public.socket_path.clone(),
            ),
            Provision::Empty => (ctx.cache_path.clone(), ctx.socket_path.clone()),
        };
        let (wake, wake_hold) = watch::channel(0);
        Ok(Arc::new(Self {
            ctx,
            cache_path,
            socket_path,
            provision: RwLock::new(provision),
            uplink: UplinkHandle::default(),
            generation: AtomicU64::new(0),
            wake,
            _wake_hold: wake_hold,
            connecting: AtomicBool::new(false),
        }))
    }

    #[cfg(test)]
    pub(crate) fn boot_isolated(dir: &Path) -> Result<Arc<Self>> {
        Self::boot_with(store::StoreCtx::isolated(dir))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    pub fn uplink(&self) -> UplinkHandle {
        self.uplink.clone()
    }

    pub fn spawn_session(
        self: &Arc<Self>,
        cache: Arc<RwLock<MessageCache>>,
        events: broadcast::Sender<Envelope>,
    ) {
        let wake = self.wake.subscribe();
        let link = Arc::clone(self);
        tokio::spawn(async move {
            session_loop(link, cache, events, wake).await;
        });
    }

    pub async fn view(&self) -> PublicView {
        let provision = self.provision.read().await;
        match &*provision {
            Provision::Empty => PublicView {
                server_url: store::peek_server_url(&self.ctx)
                    .ok()
                    .flatten()
                    .map(|u| u.as_str().to_string()),
                password_set: store::password_set(&self.ctx).unwrap_or(false),
                session: SessionState::Unconfigured,
            },
            Provision::Ready(creds) => {
                let session = if self.uplink.is_up().await {
                    SessionState::Live
                } else if self.connecting.load(Ordering::SeqCst) {
                    SessionState::Connecting
                } else {
                    SessionState::Down
                };
                PublicView {
                    server_url: Some(creds.public.server.as_str().to_string()),
                    password_set: true,
                    session,
                }
            }
        }
    }

    pub async fn apply(&self, draft: SettingsDraft) -> Result<PublicView> {
        store::commit(&self.ctx, draft)?;
        self.reconnect_from_store().await
    }

    pub async fn reconnect(&self) -> Result<PublicView> {
        self.reconnect_from_store().await
    }

    async fn reconnect_from_store(&self) -> Result<PublicView> {
        let provision = store::load(&self.ctx)?;
        let ready = matches!(provision, Provision::Ready(_));
        self.connecting.store(ready, Ordering::SeqCst);
        *self.provision.write().await = provision;
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.wake.send(gen);
        Ok(self.view().await)
    }

    pub(crate) fn set_connecting(&self, value: bool) {
        self.connecting.store(value, Ordering::SeqCst);
    }
}

pub fn commit(draft: SettingsDraft) -> Result<PublicView> {
    store::commit(&store::StoreCtx::production(), draft)
}

pub fn nudge_running() -> Result<bool> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let path = crate::config::default_socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            return Ok(false);
        }
        Err(e) => return Err(e).context("nudge unix socket"),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let req = Envelope::Req {
        id: "1".into(),
        method: "config.reconnect".into(),
        params: json!({}),
    };
    stream.write_all(format!("{}\n", req.to_line()?).as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    Ok(true)
}

pub(crate) fn merge_view(snap: &mut Value, view: &PublicView) {
    if let Some(obj) = snap.as_object_mut() {
        if let Some(fields) = view.to_status_fields().as_object() {
            for (k, v) in fields {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

pub(crate) async fn emit_sync_link(
    events: &broadcast::Sender<Envelope>,
    cache: &Arc<RwLock<MessageCache>>,
    view: &PublicView,
) {
    let mut payload = match cache.read().await.link_snapshot().await {
        Ok(v) => v,
        Err(_) => json!({}),
    };
    merge_view(&mut payload, view);
    let _ = events.send(Envelope::Event {
        topic: "sync.link".into(),
        payload,
    });
}

pub(crate) fn production_ctx() -> store::StoreCtx {
    store::StoreCtx::production()
}

async fn session_loop(
    link: Arc<Link>,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    mut wake: watch::Receiver<u64>,
) {
    loop {
        let gen = *wake.borrow();
        let provision = link.provision.read().await.clone();
        match provision {
            Provision::Empty => {
                link.set_connecting(false);
                link.uplink.detach().await;
                let _ = set_disconnected(&cache).await;
                emit_sync_link(&events, &cache, &link.view().await).await;
                if wake.changed().await.is_err() {
                    return;
                }
            }
            Provision::Ready(creds) => {
                link.set_connecting(true);
                emit_sync_link(&events, &cache, &link.view().await).await;
                crate::client::run_generation(
                    Arc::clone(&link),
                    creds,
                    Arc::clone(&cache),
                    events.clone(),
                    gen,
                    wake.clone(),
                )
                .await;
                if *wake.borrow() == gen && wake.changed().await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn set_disconnected(cache: &Arc<RwLock<MessageCache>>) -> Result<()> {
    let guard = cache.write().await;
    guard.set_meta("bridge_connected", "false").await?;
    guard.set_meta("database_ready", "false").await?;
    Ok(())
}

fn coerce_server_url(raw: &str) -> Result<ServerUrl> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("server_url is empty");
    }
    let added_scheme = !raw.contains("://");
    let with_scheme = if added_scheme {
        format!("http://{raw}")
    } else {
        raw.to_string()
    };
    let mut url = Url::parse(&with_scheme).context("server_url")?;
    if url.port().is_none() && (added_scheme || url.scheme() == "http") {
        let _ = url.set_port(Some(1234));
    }
    ServerUrl::parse(url.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_view_json_has_no_password_key() {
        let view = PublicView {
            server_url: Some("http://100.64.1.2:1234".into()),
            password_set: true,
            session: SessionState::Live,
        };
        let json = serde_json::to_string(&view).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("password").is_none());
        assert!(!value
            .as_object()
            .unwrap()
            .keys()
            .any(|k| k.eq_ignore_ascii_case("password")));
        let fields = view.to_status_fields();
        assert!(fields.get("password").is_none());
    }

    #[test]
    fn settings_draft_coerces_bare_ip() {
        let draft = SettingsDraft::from_input("100.64.1.2", None).unwrap();
        assert_eq!(draft.server.as_str(), "http://100.64.1.2:1234");
        assert!(matches!(draft.password, PasswordUpdate::Keep));

        let draft = SettingsDraft::from_input("100.64.1.2:1234", Some("")).unwrap();
        assert_eq!(draft.server.as_str(), "http://100.64.1.2:1234");
        assert!(matches!(draft.password, PasswordUpdate::Keep));

        let draft = SettingsDraft::from_input("mac.local", Some("secret")).unwrap();
        assert_eq!(draft.server.as_str(), "http://mac.local:1234");
        assert!(matches!(draft.password, PasswordUpdate::Set(_)));
    }

    #[test]
    fn password_rejects_empty_and_redacts_debug() {
        assert!(Password::new("").is_err());
        assert!(Password::new("   ").is_err());
        assert_eq!(format!("{:?}", Password::new("hidden").unwrap()), "***");
    }

    #[tokio::test]
    async fn boot_without_store_is_empty_not_err() {
        let dir = tempfile::tempdir().unwrap();
        let link = Link::boot_isolated(dir.path()).expect("missing store must not fail boot");
        let view = link.view().await;
        assert_eq!(view.session, SessionState::Unconfigured);
        assert!(!view.password_set);
        assert!(view.server_url.is_none());
    }

    #[tokio::test]
    async fn empty_store_with_url_file_still_shows_url() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "server_url = \"http://100.64.1.2:1234\"\n",
        )
        .unwrap();
        let link = Link::boot_isolated(dir.path()).unwrap();
        let view = link.view().await;
        assert_eq!(view.session, SessionState::Unconfigured);
        assert!(!view.password_set);
        assert_eq!(view.server_url.as_deref(), Some("http://100.64.1.2:1234"));
    }

    #[tokio::test]
    async fn reconnect_twice_ends_in_one_live_or_one_down_session() {
        let dir = tempfile::tempdir().unwrap();
        let link = Link::boot_isolated(dir.path()).unwrap();
        let cache = MessageCache::open(link.cache_path()).await.unwrap();
        let cache = Arc::new(RwLock::new(cache));
        let (tx, _) = broadcast::channel(16);
        link.spawn_session(Arc::clone(&cache), tx);
        let draft = SettingsDraft::from_input("http://127.0.0.1:1", Some("pw")).unwrap();
        link.apply(draft).await.unwrap();
        let first = link.reconnect().await.unwrap();
        let second = link.reconnect().await.unwrap();
        assert_eq!(first.password_set, second.password_set);
        assert_eq!(first.server_url, second.server_url);
        let settled = wait_settled(&link).await;
        assert!(
            settled.session == SessionState::Live || settled.session == SessionState::Down,
            "session {:?}",
            settled.session
        );
        let again = link.reconnect().await.unwrap();
        let settled2 = wait_settled(&link).await;
        assert_eq!(settled2.session, wait_settled(&link).await.session);
        assert!(again.password_set);
    }

    async fn wait_settled(link: &Link) -> PublicView {
        for _ in 0..80 {
            let view = link.view().await;
            if view.session != SessionState::Connecting {
                return view;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        link.view().await
    }
}
