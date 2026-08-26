use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const FORMULA: &str = "steipete/tap/imsg";
pub const INSTALL_CMD: &str = "brew install steipete/tap/imsg";

const OPT_HOMEBREW_IMSG: &str = "/opt/homebrew/bin/imsg";
const USR_LOCAL_IMSG: &str = "/usr/local/bin/imsg";
const OPT_HOMEBREW_BREW: &str = "/opt/homebrew/bin/brew";
const USR_LOCAL_BREW: &str = "/usr/local/bin/brew";

pub fn resolve_steipete_imsg(configured: &str) -> Option<PathBuf> {
    // PATH can contain this repo's cargo CLI (~/.cargo/bin/imsg), which is not steipete.
    let configured = Path::new(configured);
    if configured.is_absolute() && configured.is_file() {
        return Some(configured.to_path_buf());
    }
    for candidate in [OPT_HOMEBREW_IMSG, USR_LOCAL_IMSG] {
        let path = Path::new(candidate);
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub fn ensure_steipete_imsg(configured: &str) -> Result<PathBuf> {
    if let Some(found) = resolve_steipete_imsg(configured) {
        return Ok(found);
    }
    if let Some(brew) = resolve_brew() {
        let _ = Command::new(&brew)
            .args(["install", FORMULA])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
    }
    resolve_steipete_imsg(configured).ok_or_else(missing_error)
}

fn resolve_brew() -> Option<PathBuf> {
    for candidate in [OPT_HOMEBREW_BREW, USR_LOCAL_BREW] {
        let path = Path::new(candidate);
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join("brew");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn missing_error() -> anyhow::Error {
    let mut msg = format!("The Mac bridge needs Homebrew imsg ({FORMULA}).");
    if resolve_brew().is_none() {
        msg.push_str(" Install Homebrew from https://brew.sh, then run:\n");
    } else {
        msg.push_str(" Install it with:\n");
    }
    msg.push_str(INSTALL_CMD);
    anyhow!(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_bare_name_uses_opt_homebrew_when_present() {
        let brew = PathBuf::from(OPT_HOMEBREW_IMSG);
        if brew.is_file() {
            assert_eq!(resolve_steipete_imsg("imsg"), Some(brew));
        }
    }

    #[test]
    fn absolute_configured_file_wins_over_brew() {
        let configured = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(configured.is_file());
        assert_eq!(
            resolve_steipete_imsg(configured.to_str().unwrap()),
            Some(configured)
        );
    }

    #[test]
    fn missing_error_names_the_install_command() {
        let msg = missing_error().to_string();
        assert!(msg.contains(INSTALL_CMD), "{msg}");
        assert!(msg.contains(FORMULA), "{msg}");
    }
}
