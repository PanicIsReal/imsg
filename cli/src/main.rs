mod bridge_cmd;
mod info;
mod install;
mod setup_cmd;
mod setup_step;
mod sync_cmd;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "imsg",
    about = "Omarchy iMessage bridge — setup, sync, and serve",
    version
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Interactive setup wizard (Ink TUI)
    Setup {
        #[command(subcommand)]
        command: Option<SetupCommands>,
    },
    /// Mac bridge daemon
    Bridge {
        #[command(subcommand)]
        command: BridgeCommands,
    },
    /// Linux sync daemon
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },
    /// Install services and plugin
    Install {
        /// Path to omarchy-imessage plugin directory
        #[arg(long)]
        plugin: Option<PathBuf>,
    },
    /// Remove services and optional data
    Uninstall {
        #[arg(long)]
        purge: bool,
    },
    /// Machine-readable environment info
    Info,
    /// Run all platform checks
    Doctor,
}

#[derive(Subcommand)]
pub enum SetupCommands {
    Step {
        id: String,
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        mdns: bool,
    },
    Push {
        #[arg(long)]
        ssh: String,
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        bin: Option<PathBuf>,
        #[arg(long)]
        plugin_repo: Option<String>,
        #[arg(long)]
        enroll_port: Option<u16>,
    },
    Pair {
        code: String,
        #[arg(long)]
        host: String,
        #[arg(long, default_value_t = 18790)]
        enroll_port: u16,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        insecure: bool,
    },
    Discover {
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },
}

#[derive(clap::Args, Clone)]
pub struct BridgeArgs {
    #[command(subcommand)]
    pub command: BridgeCommands,
}

#[derive(Subcommand, Clone)]
pub enum BridgeCommands {
    Init {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 18789)]
        port: u16,
        #[arg(long)]
        mdns: bool,
    },
    Pair {
        #[arg(long)]
        rotate: bool,
    },
    ClientsImport {
        name: String,
        #[arg(long)]
        cert: PathBuf,
    },
    Serve,
    Status,
    Doctor,
}

#[derive(clap::Args, Clone)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub command: SyncCommands,
}

#[derive(Subcommand, Clone)]
pub enum SyncCommands {
    Run,
    Status,
    Request {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;

    match cli.command {
        Commands::Setup { command } => setup_cmd::run(command, json).await,
        Commands::Bridge { command } => bridge_cmd::run(command, json).await,
        Commands::Sync { command } => sync_cmd::run(command, json).await,
        Commands::Install { plugin } => install::install(plugin, json).await,
        Commands::Uninstall { purge } => install::uninstall(purge, json).await,
        Commands::Info => info::print(json),
        Commands::Doctor => doctor_all(json).await,
    }
}

async fn doctor_all(json: bool) -> Result<()> {
    let platform = info::detect();
    let mut checks = Vec::new();
    let mut ok = true;

    if platform.role == info::Role::Mac {
        let report = imsg_bridge::commands::doctor()?;
        ok &= report.ok;
        checks.extend(report.checks.into_iter().map(|c| {
            serde_json::json!({
                "scope": "bridge", "name": c.name, "ok": c.ok, "detail": c.detail
            })
        }));
    } else {
        let report = imsg_sync::commands::doctor()?;
        ok &= report.ok;
        checks.extend(report.checks.into_iter().map(|c| {
            serde_json::json!({
                "scope": "sync", "name": c.name, "ok": c.ok, "detail": c.detail
            })
        }));
        let setup = imsg_setup::doctor::run()?;
        ok &= setup.ok;
        checks.extend(setup.checks.into_iter().map(|c| {
            serde_json::json!({
                "scope": "setup", "name": c.name, "ok": c.ok, "detail": c.detail
            })
        }));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "ok": ok, "checks": checks }))?
        );
    } else {
        for c in checks {
            let mark = if c["ok"].as_bool().unwrap_or(false) {
                "✓"
            } else {
                "✗"
            };
            println!(
                "{mark} [{}] {}: {}",
                c["scope"].as_str().unwrap_or("?"),
                c["name"].as_str().unwrap_or("?"),
                c["detail"].as_str().unwrap_or("?")
            );
        }
        if !ok {
            anyhow::bail!("doctor found issues");
        }
    }
    Ok(())
}
