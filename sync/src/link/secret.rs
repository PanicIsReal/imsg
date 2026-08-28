use super::Password;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const SERVICE: &str = "imsg-sync";
const ACCOUNT: &str = "bluebubbles";
const WEBHOOK_ACCOUNT: &str = "webhook-token";

#[derive(Clone, Debug)]
pub(crate) enum SecretBackend {
    Keyring,
    #[cfg(test)]
    File(PathBuf),
}

impl SecretBackend {
    #[cfg(test)]
    pub(crate) fn file_sibling(config_path: &Path) -> Self {
        let mut name = config_path.as_os_str().to_owned();
        name.push(".secret");
        Self::File(PathBuf::from(name))
    }
}

pub(crate) fn load(backend: &SecretBackend) -> Result<Option<Password>> {
    match backend {
        SecretBackend::Keyring => load_keyring(),
        #[cfg(test)]
        SecretBackend::File(path) => load_file(path),
    }
}

pub(crate) fn store(backend: &SecretBackend, password: &Password) -> Result<()> {
    match backend {
        SecretBackend::Keyring => store_keyring(password),
        #[cfg(test)]
        SecretBackend::File(path) => store_file(path, password),
    }
}

pub(crate) fn load_webhook(backend: &SecretBackend) -> Result<Option<Password>> {
    match backend {
        SecretBackend::Keyring => load_keyring_named(WEBHOOK_ACCOUNT),
        #[cfg(test)]
        SecretBackend::File(path) => load_file(&webhook_file(path)),
    }
}

pub(crate) fn store_webhook(backend: &SecretBackend, token: &Password) -> Result<()> {
    match backend {
        SecretBackend::Keyring => store_keyring_named(WEBHOOK_ACCOUNT, token),
        #[cfg(test)]
        SecretBackend::File(path) => store_file(&webhook_file(path), token),
    }
}

#[cfg(test)]
fn webhook_file(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".webhook");
    PathBuf::from(name)
}

fn load_keyring() -> Result<Option<Password>> {
    load_keyring_named(ACCOUNT)
}

fn store_keyring(password: &Password) -> Result<()> {
    store_keyring_named(ACCOUNT, password)
}

fn load_keyring_named(account: &str) -> Result<Option<Password>> {
    let entry = keyring::Entry::new(SERVICE, account).context("keyring entry")?;
    match entry.get_password() {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(Password::new(raw)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("keyring load"),
    }
}

fn store_keyring_named(account: &str, password: &Password) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, account).context("keyring entry")?;
    entry
        .set_password(password.as_str())
        .context("keyring store")
}

#[cfg(test)]
fn load_file(path: &Path) -> Result<Option<Password>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).context("read secret file")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(Password::new(trimmed)?))
}

#[cfg(test)]
fn store_file(path: &Path, password: &Password) -> Result<()> {
    write_private(path, password.as_str().as_bytes())
}

pub(crate) fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, contents.as_ref()).context("write private tmp")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).context("rename private file")?;
    Ok(())
}
