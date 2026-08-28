use super::secret::{self, SecretBackend};
use super::{
    Credentials, Password, PasswordUpdate, Provision, PublicConfig, PublicView, SessionState,
    SettingsDraft, WebhookDraft,
};
use crate::config::{default_socket_path, ServerUrl};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct StoreCtx {
    pub config_path: PathBuf,
    pub secret: SecretBackend,
    pub cache_path: PathBuf,
    pub socket_path: PathBuf,
}

impl StoreCtx {
    pub(crate) fn production() -> Self {
        Self {
            config_path: default_config_path(),
            secret: SecretBackend::Keyring,
            cache_path: default_cache_path(),
            socket_path: default_socket_path(),
        }
    }

    #[cfg(test)]
    pub(crate) fn isolated(dir: &Path) -> Self {
        let config_path = dir.join("config.toml");
        Self {
            secret: SecretBackend::file_sibling(&config_path),
            config_path,
            cache_path: dir.join("cache.db"),
            socket_path: dir.join("imsg-sync.sock"),
        }
    }
}

#[derive(Serialize)]
struct PublicFile {
    server_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    socket_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefetch_chats: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefetch_messages: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    webhook_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_serve_url: Option<String>,
}

#[derive(Deserialize)]
struct FileIn {
    server_url: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    cache_path: Option<PathBuf>,
    #[serde(default)]
    socket_path: Option<PathBuf>,
    #[serde(default)]
    prefetch_chats: Option<u32>,
    #[serde(default)]
    prefetch_messages: Option<u32>,
    #[serde(default)]
    webhook_enabled: bool,
    #[serde(default)]
    webhook_port: Option<u16>,
    #[serde(default)]
    webhook_serve_url: Option<String>,
}

pub(crate) fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("imsg-sync")
        .join("config.toml")
}

pub(crate) fn default_cache_path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("omarchy-imessage")
        .join("cache.db")
}

impl PublicFile {
    fn from_in(file: &FileIn) -> Self {
        Self {
            server_url: file.server_url.clone().unwrap_or_default(),
            cache_path: file.cache_path.clone(),
            socket_path: file.socket_path.clone(),
            prefetch_chats: file.prefetch_chats,
            prefetch_messages: file.prefetch_messages,
            webhook_enabled: file.webhook_enabled,
            webhook_port: file.webhook_port,
            webhook_serve_url: file.webhook_serve_url.clone(),
        }
    }
}

pub(crate) fn load(ctx: &StoreCtx) -> Result<Provision> {
    if !ctx.config_path.exists() {
        return Ok(Provision::Empty);
    }
    let text = std::fs::read_to_string(&ctx.config_path).context("read sync config")?;
    let file: FileIn = toml::from_str(&text).context("parse sync config")?;
    if let Some(raw) = file
        .password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        secret::store(&ctx.secret, &Password::new(raw)?)?;
        write_public(&ctx.config_path, &PublicFile::from_in(&file))?;
    }
    let Some(raw_url) = file
        .server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(Provision::Empty);
    };
    let Ok(server) = ServerUrl::parse(raw_url) else {
        return Ok(Provision::Empty);
    };
    let Some(secret) = secret::load(&ctx.secret)? else {
        return Ok(Provision::Empty);
    };
    Ok(Provision::Ready(Credentials::new(
        public_from_file(ctx, server, &file),
        secret,
    )))
}

pub(crate) fn commit(ctx: &StoreCtx, draft: SettingsDraft) -> Result<PublicView> {
    match &draft.password {
        PasswordUpdate::Keep => {
            if secret::load(&ctx.secret)?.is_none() {
                bail!("password is not set; provide a password");
            }
        }
        PasswordUpdate::Set(password) => {
            secret::store(&ctx.secret, password)?;
        }
    }

    let mut public = if ctx.config_path.exists() {
        let text = std::fs::read_to_string(&ctx.config_path).context("read sync config")?;
        let file: FileIn = toml::from_str(&text).context("parse sync config")?;
        PublicFile::from_in(&file)
    } else {
        PublicFile {
            server_url: String::new(),
            cache_path: None,
            socket_path: None,
            prefetch_chats: None,
            prefetch_messages: None,
            webhook_enabled: false,
            webhook_port: None,
            webhook_serve_url: None,
        }
    };
    public.server_url = draft.server.as_str().to_string();
    write_public(&ctx.config_path, &public)?;
    Ok(view_from_provision(&load(ctx)?))
}

pub(crate) fn view_from_provision(provision: &Provision) -> PublicView {
    match provision {
        Provision::Empty => PublicView {
            server_url: None,
            password_set: false,
            session: SessionState::Unconfigured,
            webhook_enabled: false,
            webhook_port: crate::webhook::DEFAULT_PORT,
            webhook_serve_url: String::new(),
            webhook_listening: false,
            webhook_registered: false,
            webhook_token_set: false,
        },
        Provision::Ready(creds) => PublicView {
            server_url: Some(creds.public.server.as_str().to_string()),
            password_set: true,
            session: SessionState::Down,
            webhook_enabled: creds.public.webhook_enabled,
            webhook_port: creds.public.webhook_port,
            webhook_serve_url: creds.public.webhook_serve_url.clone(),
            webhook_listening: false,
            webhook_registered: false,
            webhook_token_set: false,
        },
    }
}

pub(crate) fn password_set(ctx: &StoreCtx) -> Result<bool> {
    Ok(secret::load(&ctx.secret)?.is_some())
}

pub(crate) fn peek_server_url(ctx: &StoreCtx) -> Result<Option<ServerUrl>> {
    if !ctx.config_path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&ctx.config_path).context("read sync config")?;
    let file: FileIn = toml::from_str(&text).context("parse sync config")?;
    let Some(raw) = file
        .server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(ServerUrl::parse(raw)?))
}

fn public_from_file(ctx: &StoreCtx, server: ServerUrl, file: &FileIn) -> PublicConfig {
    PublicConfig {
        server,
        cache_path: file.cache_path.clone().unwrap_or_else(|| ctx.cache_path.clone()),
        socket_path: file
            .socket_path
            .clone()
            .unwrap_or_else(|| ctx.socket_path.clone()),
        prefetch_chats: file.prefetch_chats.unwrap_or(40),
        prefetch_messages: file.prefetch_messages.unwrap_or(100),
        webhook_enabled: file.webhook_enabled,
        webhook_port: file.webhook_port.unwrap_or(crate::webhook::DEFAULT_PORT),
        webhook_serve_url: file.webhook_serve_url.clone().unwrap_or_default(),
    }
}

#[derive(Clone, Default)]
pub(crate) struct WebhookSnap {
    pub enabled: bool,
    pub port: u16,
    pub serve_url: String,
}

pub(crate) fn peek_webhook(ctx: &StoreCtx) -> Result<WebhookSnap> {
    if !ctx.config_path.exists() {
        return Ok(WebhookSnap {
            enabled: false,
            port: crate::webhook::DEFAULT_PORT,
            serve_url: String::new(),
        });
    }
    let text = std::fs::read_to_string(&ctx.config_path).context("read sync config")?;
    let file: FileIn = toml::from_str(&text).context("parse sync config")?;
    Ok(WebhookSnap {
        enabled: file.webhook_enabled,
        port: file.webhook_port.unwrap_or(crate::webhook::DEFAULT_PORT),
        serve_url: file.webhook_serve_url.unwrap_or_default(),
    })
}

pub(crate) fn webhook_token_set(ctx: &StoreCtx) -> Result<bool> {
    Ok(secret::load_webhook(&ctx.secret)?.is_some())
}

pub(crate) fn ensure_webhook_token(ctx: &StoreCtx) -> Result<Password> {
    if let Some(existing) = secret::load_webhook(&ctx.secret)? {
        return Ok(existing);
    }
    let token = Password::new(crate::webhook::generate_token())?;
    secret::store_webhook(&ctx.secret, &token)?;
    Ok(token)
}

pub(crate) fn commit_webhook(ctx: &StoreCtx, draft: WebhookDraft) -> Result<PublicView> {
    if draft.port == 0 {
        bail!("webhook port must be non-zero");
    }
    let mut public = if ctx.config_path.exists() {
        let text = std::fs::read_to_string(&ctx.config_path).context("read sync config")?;
        let file: FileIn = toml::from_str(&text).context("parse sync config")?;
        PublicFile::from_in(&file)
    } else {
        PublicFile {
            server_url: String::new(),
            cache_path: None,
            socket_path: None,
            prefetch_chats: None,
            prefetch_messages: None,
            webhook_enabled: false,
            webhook_port: None,
            webhook_serve_url: None,
        }
    };
    public.webhook_enabled = draft.enabled;
    public.webhook_port = Some(draft.port);
    let serve = draft.serve_url.trim().to_string();
    public.webhook_serve_url = if serve.is_empty() { None } else { Some(serve) };
    if draft.enabled {
        let _ = ensure_webhook_token(ctx)?;
    }
    write_public(&ctx.config_path, &public)?;
    Ok(view_from_provision(&load(ctx)?))
}

pub(crate) fn rotate_webhook_token(ctx: &StoreCtx) -> Result<()> {
    let token = Password::new(crate::webhook::generate_token())?;
    secret::store_webhook(&ctx.secret, &token)?;
    Ok(())
}

pub(crate) fn webhook_copy_url(ctx: &StoreCtx) -> Result<String> {
    let snap = peek_webhook(ctx)?;
    let origin = if snap.serve_url.trim().is_empty() {
        crate::webhook::guess_serve_origin().unwrap_or_else(|| format!("http://127.0.0.1:{}", snap.port))
    } else {
        snap.serve_url
    };
    let token = ensure_webhook_token(ctx)?;
    crate::webhook::hook_url(&origin, token.as_str())
}

fn write_public(path: &Path, file: &PublicFile) -> Result<()> {
    let body = toml::to_string_pretty(file).context("serialize public config")?;
    secret::write_private(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_file_toml_has_no_password_key() {
        let file = PublicFile {
            server_url: "http://100.64.1.2:1234".into(),
            cache_path: None,
            socket_path: None,
            prefetch_chats: Some(20),
            prefetch_messages: Some(50),
            webhook_enabled: true,
            webhook_port: Some(18792),
            webhook_serve_url: Some("https://box.ts.net".into()),
        };
        let text = toml::to_string(&file).unwrap();
        assert!(
            !text.to_lowercase().contains("password"),
            "public toml leaked a password key: {text}"
        );
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert!(parsed.get("password").is_none());
        assert!(
            !text.contains("token"),
            "public toml leaked a token: {text}"
        );
    }

    #[test]
    fn load_migrates_plaintext_password_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = StoreCtx::isolated(dir.path());
        std::fs::write(
            &ctx.config_path,
            "server_url = \"http://100.64.1.2:1234\"\npassword = \"s3cret\"\n",
        )
        .unwrap();

        let first = load(&ctx).unwrap();
        match first {
            Provision::Ready(creds) => {
                assert_eq!(creds.public.server.as_str(), "http://100.64.1.2:1234");
                assert_eq!(creds.password().as_str(), "s3cret");
            }
            Provision::Empty => panic!("expected Ready after migrate"),
        }

        let rewritten = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(
            !rewritten.to_lowercase().contains("password"),
            "migrated toml still has password: {rewritten}"
        );
        let parsed: toml::Value = toml::from_str(&rewritten).unwrap();
        assert!(parsed.get("password").is_none());

        let secret_path = match &ctx.secret {
            SecretBackend::File(path) => path.clone(),
            SecretBackend::Keyring => panic!("isolated store must use file secret"),
        };
        let mode = std::fs::metadata(&secret_path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(mode.mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_to_string(&secret_path).unwrap().trim(), "s3cret");

        let second = load(&ctx).unwrap();
        match second {
            Provision::Ready(creds) => {
                assert_eq!(creds.password().as_str(), "s3cret");
            }
            Provision::Empty => panic!("second load should stay Ready"),
        }
        let again = std::fs::read_to_string(&ctx.config_path).unwrap();
        assert!(!again.to_lowercase().contains("password"));
    }

    #[test]
    fn commit_keep_without_secret_fails() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = StoreCtx::isolated(dir.path());
        let draft = SettingsDraft::from_input("http://100.64.1.2:1234", None).unwrap();
        let err = commit(&ctx, draft).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("password"),
            "{err}"
        );
        assert!(!ctx.config_path.exists());
    }
}
