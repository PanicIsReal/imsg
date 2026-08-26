use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepId {
    Detect,
    Brew,
    Certs,
    Service,
    Enroll,
}

impl FromStr for StepId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "detect" => Ok(Self::Detect),
            "brew" => Ok(Self::Brew),
            "certs" => Ok(Self::Certs),
            "service" => Ok(Self::Service),
            "enroll" => Ok(Self::Enroll),
            other => bail!("unknown setup step: {other}"),
        }
    }
}

impl StepId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Detect => "detect",
            Self::Brew => "brew",
            Self::Certs => "certs",
            Self::Service => "service",
            Self::Enroll => "enroll",
        }
    }
}

pub fn refuse_wildcard_bind(bind: &str) -> Result<()> {
    let ip: IpAddr = bind
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid bind address: {bind}"))?;
    if ip.is_unspecified() {
        bail!("refusing to bind to unspecified address (0.0.0.0)");
    }
    Ok(())
}

pub fn wait_enroll(bind: &str, port: u16, timeout: Duration) -> Result<()> {
    let target = format!("{bind}:{port}");
    let addrs: Vec<_> = target
        .to_socket_addrs()
        .map_err(|e| anyhow::anyhow!("resolve {target}: {e}"))?
        .collect();
    if addrs.is_empty() {
        bail!("no addresses for {target}");
    }
    let deadline = Instant::now() + timeout;
    let mut last = None;
    while Instant::now() < deadline {
        if lsof_listening(port) {
            return Ok(());
        }
        match TcpStream::connect_timeout(&addrs[0], Duration::from_millis(400)) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    bail!(
        "enroll did not listen on {target}: {}",
        last.map(|e| e.to_string()).unwrap_or_else(|| "timeout".into())
    )
}

fn lsof_listening(port: u16) -> bool {
    for bin in ["/usr/sbin/lsof", "/usr/bin/lsof", "lsof"] {
        if Command::new(bin)
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_step_ids() {
        assert_eq!("detect".parse::<StepId>().unwrap(), StepId::Detect);
        assert_eq!("brew".parse::<StepId>().unwrap(), StepId::Brew);
        assert_eq!("certs".parse::<StepId>().unwrap(), StepId::Certs);
        assert_eq!("service".parse::<StepId>().unwrap(), StepId::Service);
        assert_eq!("enroll".parse::<StepId>().unwrap(), StepId::Enroll);
        assert!("nope".parse::<StepId>().is_err());
        assert_eq!(StepId::Certs.as_str(), "certs");
    }

    #[test]
    fn refuses_unspecified_bind() {
        assert!(refuse_wildcard_bind("0.0.0.0").is_err());
        assert!(refuse_wildcard_bind("127.0.0.1").is_ok());
        assert!(refuse_wildcard_bind("100.64.1.2").is_ok());
    }
}
