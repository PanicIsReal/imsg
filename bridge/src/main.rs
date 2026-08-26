use anyhow::Result;
use clap::Parser;
use imsg_bridge::commands;
use imsg_bridge::install_crypto_provider;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "imsg-bridge", about = "Deprecated: use `imsg bridge`")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Init {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 18789)]
        port: u16,
        #[arg(long)]
        mdns: bool,
        #[arg(long)]
        json: bool,
    },
    Pair {
        #[arg(long)]
        json: bool,
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
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("note: imsg-bridge is deprecated; use `imsg bridge` instead");
    install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("imsg_bridge=info".parse()?))
        .init();

    match Cli::parse().command {
        Commands::Init {
            bind,
            port,
            mdns,
            json,
        } => {
            let result = commands::init(bind, port, mdns)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("Initialized at {:?}", result.data_dir);
                println!("Pairing code: {}", result.pairing_code);
                println!("Enroll port: {}", result.enroll_port);
                if mdns {
                    println!("mDNS advertise: enabled");
                }
                println!("Run: imsg bridge serve");
            }
        }
        Commands::Pair { json, rotate } => {
            if let Some(v) = commands::pair(json, rotate)? {
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        Commands::ClientsImport { name, cert } => {
            let path = commands::import_client(name, cert)?;
            println!("Imported client cert to {:?}", path);
        }
        Commands::Serve => commands::serve().await?,
        Commands::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&commands::status().await?)?
            );
        }
        Commands::Doctor { json } => {
            let report = commands::doctor()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for c in &report.checks {
                    let mark = if c.ok { "✓" } else { "✗" };
                    println!("{mark} {}: {}", c.name, c.detail);
                }
                if !report.ok {
                    anyhow::bail!("doctor found issues");
                }
            }
        }
    }
    Ok(())
}
