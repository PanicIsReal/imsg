use crate::paths::SyncPaths;
use anyhow::Result;
use serde::Serialize;
use std::fs;

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

pub fn run() -> Result<DoctorReport> {
    let paths = SyncPaths::default_paths();
    let mut checks = Vec::new();
    let mut ok = true;

    let mut push = |name: &str, pass: bool, detail: &str| {
        checks.push(DoctorCheck {
            name: name.into(),
            ok: pass,
            detail: detail.into(),
        });
        pass
    };

    ok &= push(
        "config",
        paths.config_path.exists(),
        &paths.config_path.display().to_string(),
    );
    ok &= push(
        "ca",
        paths.state_dir.join("ca.pem").exists(),
        "ca.pem present",
    );
    ok &= push(
        "client-cert",
        paths.state_dir.join("client.pem").exists(),
        "client.pem present",
    );
    ok &= push(
        "client-key",
        paths.state_dir.join("client-key.pem").exists(),
        "client-key.pem present",
    );

    if paths.config_path.exists() {
        let text = fs::read_to_string(&paths.config_path)?;
        let parsed: toml::Table = toml::from_str(&text)?;
        if let Some(url) = parsed.get("bridge_url").and_then(|v| v.as_str()) {
            ok &= push("bridge-url", !url.is_empty(), url);
        }
    }

    Ok(DoctorReport { ok, checks })
}

pub fn run_human() -> Result<()> {
    let report = run()?;
    for c in &report.checks {
        let mark = if c.ok { "✓" } else { "✗" };
        println!("{mark} {}: {}", c.name, c.detail);
    }
    if !report.ok {
        anyhow::bail!("doctor found issues — run `imsg setup pair <code> --host <mac>`");
    }
    println!("Ready to run: imsg sync run");
    Ok(())
}
