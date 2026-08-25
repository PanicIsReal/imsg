use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::path::{Component, Path, PathBuf};

type HmacSha256 = Hmac<Sha256>;

const ATTACHMENTS_ROOT: &str = "Library/Messages/Attachments";
const MAX_BYTES: u64 = 25 * 1024 * 1024;

pub fn token_for(chat_guid: &str, message_guid: &str, filename: &str, secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(format!("{chat_guid}:{message_guid}:{filename}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn resolve_attachment(home: &Path, relative: &str) -> Result<PathBuf> {
    let root = home.join(ATTACHMENTS_ROOT);
    let candidate = root.join(relative);
    let canonical = candidate.canonicalize().context("canonicalize attachment")?;
    let root_canon = root.canonicalize().unwrap_or(root);
    if !canonical.starts_with(&root_canon) {
        anyhow::bail!("attachment path outside allowed root");
    }
    for component in canonical.components() {
        if matches!(component, Component::ParentDir) {
            anyhow::bail!("path traversal rejected");
        }
    }
    let meta = std::fs::metadata(&canonical)?;
    if meta.len() > MAX_BYTES {
        anyhow::bail!("attachment exceeds size cap");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_stable() {
        let a = token_for("c", "m", "f.jpg", b"secret");
        let b = token_for("c", "m", "f.jpg", b"secret");
        assert_eq!(a, b);
    }
}
