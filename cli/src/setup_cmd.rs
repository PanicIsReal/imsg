use crate::{setup_step, SetupCommands};
use anyhow::{Context, Result};
use imsg_setup::push::{PushSpec, DEFAULT_PLUGIN_REPO};
use imsg_setup::step::StepId;
use std::path::PathBuf;
use std::process::Command;

pub async fn run(command: Option<SetupCommands>, json: bool) -> Result<()> {
    match command {
        None => launch_tui(),
        Some(SetupCommands::Step { id, bind, mdns }) => {
            let id: StepId = id.parse()?;
            setup_step::run(id, bind, mdns, json)
        }
        Some(SetupCommands::Push {
            ssh,
            code,
            bind,
            bin,
            plugin_repo,
            enroll_port,
        }) => {
            let config = imsg_bridge::config::Config::load().ok();
            let code = code
                .or_else(|| config.as_ref().and_then(|c| c.pairing_code.clone()))
                .context("pairing code missing; finish Mac setup first")?;
            let bind = bind
                .or_else(|| config.as_ref().map(|c| c.bind.clone()))
                .context("Mac bind missing; finish Mac setup first")?;
            let enroll_port = enroll_port
                .or_else(|| config.as_ref().map(|c| c.enroll_port))
                .unwrap_or(18790);
            let spec = PushSpec {
                ssh,
                code,
                bind,
                enroll_port,
                bin,
                plugin_repo: plugin_repo.unwrap_or_else(|| DEFAULT_PLUGIN_REPO.into()),
            };
            let result = imsg_setup::push::push(spec)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.detail);
            }
            Ok(())
        }
        Some(SetupCommands::Pair {
            code,
            host,
            enroll_port,
            name,
            insecure,
        }) => {
            if json {
                let result =
                    imsg_setup::enroll::pair(&code, &host, enroll_port, name.as_deref(), insecure)
                        .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                imsg_setup::enroll::pair_human(
                    &code,
                    &host,
                    enroll_port,
                    name.as_deref(),
                    insecure,
                )
                .await?;
            }
            Ok(())
        }
        Some(SetupCommands::Connect { url, password }) => {
            imsg_sync::install_crypto_provider();
            let cfg = imsg_sync::config::SyncConfig::from_parts(&url, &password)?;
            imsg_sync::bb::BlueBubbles::connect(cfg.clone())
                .await
                .with_context(|| format!("ping {}", cfg.server.as_str()))?;
            cfg.save()?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "server_url": cfg.server.as_str(),
                        "config": imsg_sync::config::SyncConfig::path().display().to_string(),
                    })
                );
            } else {
                println!("wrote {}", imsg_sync::config::SyncConfig::path().display());
                println!("start sync with: imsg sync run");
            }
            Ok(())
        }
        Some(SetupCommands::Discover { timeout }) => {
            if json {
                let result = imsg_setup::discover::browse(timeout).await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                imsg_setup::discover::browse_human(timeout).await?;
            }
            Ok(())
        }
    }
}

fn launch_tui() -> Result<()> {
    let exe = std::env::current_exe().ok();
    let imsg_bin = exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "imsg".into());

    if let Some(exe) = &exe {
        let sibling = exe.with_file_name("imsg-tui");
        if sibling.is_file() {
            return exec_tui(sibling, &imsg_bin);
        }
    }

    if let Ok(path) = std::env::var("IMSG_TUI") {
        let candidate = PathBuf::from(&path);
        if candidate.is_file() {
            return exec_tui(candidate, &imsg_bin);
        }
    }

    let mut tui_dir: Option<PathBuf> = None;
    if let Some(exe) = &exe {
        for ancestor in exe.ancestors() {
            let candidate = ancestor.join("tui");
            if candidate.join("package.json").exists() {
                tui_dir = Some(candidate);
                break;
            }
        }
    }
    if tui_dir.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join("tui");
            if candidate.join("package.json").exists() {
                tui_dir = Some(candidate);
            }
        }
    }

    if let Some(dir) = tui_dir {
        let status = Command::new("npm")
            .args(["run", "start", "--silent"])
            .current_dir(&dir)
            .env("IMSG_BIN", &imsg_bin)
            .status();
        match status {
            Ok(s) if s.success() => return Ok(()),
            Ok(_) => anyhow::bail!("setup TUI exited with error"),
            Err(e) => anyhow::bail!("failed to launch TUI (is Node.js installed?): {e}"),
        }
    }

    anyhow::bail!(
        "Interactive setup needs imsg-tui next to this binary, IMSG_TUI, or a tui/ checkout."
    )
}

fn exec_tui(path: PathBuf, imsg_bin: &str) -> Result<()> {
    let status = Command::new(&path)
        .env("IMSG_BIN", imsg_bin)
        .status()
        .with_context(|| format!("launch {}", path.display()))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("setup TUI exited with error")
    }
}
