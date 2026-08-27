use anyhow::{bail, Context, Result};
use url::Url;

#[derive(Clone)]
pub struct ServerUrl(Url);

impl ServerUrl {
    pub fn parse(raw: &str) -> Result<Self> {
        let url = Url::parse(raw.trim()).context("server_url")?;
        match url.scheme() {
            "http" | "https" => {}
            other => bail!("server_url must be http or https, not {other}"),
        }
        if url.host_str().is_none() {
            bail!("server_url needs a host");
        }
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    pub fn join(&self, path: &str) -> Result<Url> {
        self.0
            .join(path.trim_start_matches('/'))
            .context("join server path")
    }
}

impl std::fmt::Debug for ServerUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn default_socket_path() -> std::path::PathBuf {
    let uid = unsafe { libc::getuid() };
    std::path::PathBuf::from(format!("/run/user/{uid}/imsg-sync.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wss() {
        assert!(ServerUrl::parse("wss://mac:1234").is_err());
        assert!(ServerUrl::parse("http://100.64.1.2:1234").is_ok());
    }
}
