mod attachments;
mod config;
mod imsg_rpc;
mod server;
mod tls;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "imsg-bridge", about = "Mac-side iMessage bridge for Omarchy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create config and TLS material
    Init {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 18789)]
        port: u16,
    },
    /// Print pairing code and CA cert path
    Pair,
    /// Import a client certificate PEM
    ClientsImport {
        name: String,
        #[arg(long)]
        cert: PathBuf,
    },
    /// Run the bridge server
    Serve,
    /// Print imsg status via local rpc
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("imsg_bridge=info".parse()?))
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init { bind, port } => cmd_init(bind, port),
        Commands::Pair => cmd_pair(),
        Commands::ClientsImport { name, cert } => cmd_import_client(name, cert),
        Commands::Serve => cmd_serve().await,
        Commands::Status => cmd_status().await,
    }
}

fn cmd_init(bind: String, port: u16) -> Result<()> {
    let mut config = Config::default();
    config.bind = bind;
    config.port = port;
    if config.pairing_code.is_none() {
        config.pairing_code = Some(uuid::Uuid::new_v4().simple().to_string()[..8].to_string());
    }
    config.save()?;
    tls::init_certs(&config.data_dir)?;
    println!("Initialized at {:?}", config.data_dir);
    println!("Pairing code: {}", config.pairing_code.as_deref().unwrap_or("?"));
    println!("Run: imsg-bridge serve");
    Ok(())
}

fn cmd_pair() -> Result<()> {
    let config = Config::load()?;
    println!("CA cert: {:?}", config.data_dir.join("ca.pem"));
    println!(
        "Pairing code: {}",
        config.pairing_code.as_deref().unwrap_or("(run init first)")
    );
    println!("Import client cert: imsg-bridge clients-import <name> --cert client.pem");
    Ok(())
}

fn cmd_import_client(name: String, cert: PathBuf) -> Result<()> {
    let config = Config::load()?;
    let pem = std::fs::read_to_string(&cert).context("read client cert")?;
    let path = tls::import_client_cert(&config.data_dir, &name, &pem)?;
    println!("Imported client cert to {:?}", path);
    Ok(())
}

async fn cmd_serve() -> Result<()> {
    let config = Config::load()?;
    config.validate_bind()?;
    let tls_mat = tls::load_server_config(&config.data_dir)?;
    let rustls_config =
        axum_server::tls_rustls::RustlsConfig::from_config(tls_mat.server_config.clone());
    server::run(config, rustls_config).await
}

async fn cmd_status() -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let rpc = imsg_rpc::ImsgRpc::spawn(&config.imsg_path).await?;
    match rpc.status().await {
        Ok(status) => println!("{}", serde_json::to_string_pretty(&status)?),
        Err(_) => {
            let chats = rpc.call("chats.list", serde_json::json!({"limit": 1})).await?;
            println!("{}", serde_json::to_string_pretty(&chats)?);
        }
    }
    Ok(())
}
