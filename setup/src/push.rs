use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const DEFAULT_PLUGIN_REPO: &str = "https://github.com/PanicIsReal/omarchy-imessage.git";
pub const DEFAULT_LINUX_ASSET: &str =
    "https://github.com/PanicIsReal/imsg/releases/latest/download/imsg-linux-x86_64.tar.gz";

#[derive(Debug, Clone)]
pub struct PushSpec {
    pub ssh: String,
    pub code: String,
    pub bind: String,
    pub enroll_port: u16,
    pub bin: Option<PathBuf>,
    pub plugin_repo: String,
}

#[derive(Debug, Serialize)]
pub struct PushStep {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct PushResult {
    pub ok: bool,
    pub ssh: String,
    pub client_name: Option<String>,
    pub detail: String,
    pub steps: Vec<PushStep>,
}

pub fn remote_argv(ssh: &str, remote: &[&str]) -> Vec<String> {
    let mut args = vec!["ssh".into(), ssh.into()];
    args.extend(remote.iter().map(|s| (*s).to_string()));
    args
}

pub fn install_remote_bin_argv(spec: &PushSpec) -> Vec<Vec<String>> {
    if let Some(bin) = &spec.bin {
        return vec![
            vec![
                "scp".into(),
                bin.display().to_string(),
                format!("{}:.local/bin/imsg", spec.ssh),
            ],
            remote_argv(&spec.ssh, &["chmod", "+x", ".local/bin/imsg"]),
        ];
    }
    vec![
        remote_argv(&spec.ssh, &["mkdir", "-p", ".local/bin"]),
        remote_argv(
            &spec.ssh,
            &[
                "curl",
                "-fsSL",
                "-o",
                "/tmp/imsg-linux-x86_64.tar.gz",
                DEFAULT_LINUX_ASSET,
            ],
        ),
        remote_argv(
            &spec.ssh,
            &[
                "tar",
                "-xzf",
                "/tmp/imsg-linux-x86_64.tar.gz",
                "-C",
                "/tmp",
            ],
        ),
        remote_argv(
            &spec.ssh,
            &[
                "install",
                "-Dm755",
                "/tmp/imsg",
                ".local/bin/imsg",
            ],
        ),
    ]
}

pub fn pair_remote_argv(spec: &PushSpec) -> Vec<String> {
    let port = spec.enroll_port.to_string();
    remote_argv(
        &spec.ssh,
        &[
            ".local/bin/imsg",
            "setup",
            "pair",
            &spec.code,
            "--host",
            &spec.bind,
            "--enroll-port",
            &port,
            "--insecure",
        ],
    )
}

pub fn service_remote_argv(spec: &PushSpec) -> Vec<String> {
    remote_argv(&spec.ssh, &[".local/bin/imsg", "install"])
}

pub fn plugin_remote_argv(spec: &PushSpec) -> Vec<String> {
    remote_argv(
        &spec.ssh,
        &[
            "omarchy",
            "plugin",
            "add",
            &spec.plugin_repo,
            "--enable",
        ],
    )
}

fn run_line(argv: &[String]) -> Result<String> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::inherit());
    let out = cmd.output().with_context(|| format!("spawn {}", argv[0]))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        anyhow::bail!(
            "{} failed: {}",
            argv.join(" "),
            if stderr.is_empty() { stdout } else { stderr }
        );
    }
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

pub fn push(spec: PushSpec) -> Result<PushResult> {
    if spec.code.trim().is_empty() {
        anyhow::bail!("pairing code is required");
    }
    if spec.bind.trim().is_empty() {
        anyhow::bail!("Mac bind/host is required");
    }

    let _ = Command::new("ssh")
        .args([&spec.ssh, "mkdir", "-p", ".local/bin"])
        .status();

    let mut steps = Vec::new();
    for line in install_remote_bin_argv(&spec) {
        run_line(&line)?;
    }
    steps.push(PushStep {
        id: "ssh-bin".into(),
        ok: true,
        detail: "linux imsg installed".into(),
    });

    let pair_out = run_line(&pair_remote_argv(&spec))?;
    let client_name = serde_json::from_str::<serde_json::Value>(&pair_out)
        .ok()
        .and_then(|v| {
            v.get("client_name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        });
    steps.push(PushStep {
        id: "ssh-pair".into(),
        ok: true,
        detail: client_name
            .clone()
            .unwrap_or_else(|| "paired".into()),
    });

    run_line(&service_remote_argv(&spec))?;
    steps.push(PushStep {
        id: "ssh-service".into(),
        ok: true,
        detail: "imsg install".into(),
    });

    let plugin = match run_line(&plugin_remote_argv(&spec)) {
        Ok(out) => PushStep {
            id: "ssh-plugin".into(),
            ok: true,
            detail: if out.is_empty() {
                "omarchy plugin enabled".into()
            } else {
                out
            },
        },
        Err(e) => {
            let has_omarchy = run_line(&remote_argv(&spec.ssh, &["which", "omarchy"])).is_ok();
            PushStep {
                id: "ssh-plugin".into(),
                ok: !has_omarchy,
                detail: if has_omarchy {
                    e.to_string()
                } else {
                    "omarchy not installed; skipped".into()
                },
            }
        }
    };
    let plugin_ok = plugin.ok;
    let plugin_detail = plugin.detail.clone();
    steps.push(plugin);

    if !plugin_ok {
        anyhow::bail!("{}", plugin_detail);
    }

    Ok(PushResult {
        ok: true,
        ssh: spec.ssh,
        client_name,
        detail: format!("paired; {plugin_detail}"),
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PushSpec {
        PushSpec {
            ssh: "omarchy".into(),
            code: "abcd1234".into(),
            bind: "100.64.1.2".into(),
            enroll_port: 18790,
            bin: None,
            plugin_repo: DEFAULT_PLUGIN_REPO.into(),
        }
    }

    #[test]
    fn pair_argv_is_not_a_shell_string() {
        let argv = pair_remote_argv(&spec());
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[1], "omarchy");
        assert!(argv.contains(&"setup".into()));
        assert!(argv.contains(&"pair".into()));
        assert!(argv.contains(&"abcd1234".into()));
        assert!(argv.contains(&"100.64.1.2".into()));
        assert!(argv.contains(&"--host".into()));
        assert!(argv.contains(&"--insecure".into()));
        assert!(!argv.iter().any(|a| a.contains(" && ")));
    }

    #[test]
    fn release_download_uses_named_asset() {
        let lines = install_remote_bin_argv(&spec());
        let flat: Vec<String> = lines.into_iter().flatten().collect();
        assert!(flat.iter().any(|a| a == DEFAULT_LINUX_ASSET));
    }

    #[test]
    fn local_bin_uses_scp() {
        let mut s = spec();
        s.bin = Some(PathBuf::from("/tmp/imsg-linux"));
        let lines = install_remote_bin_argv(&s);
        assert_eq!(lines[0][0], "scp");
        assert_eq!(lines[0][1], "/tmp/imsg-linux");
    }
}
