mod cache;
mod client;
mod config;
mod socket_server;
mod uplink;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

fn install_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");
}

#[derive(Parser)]
#[command(name = "imsg-sync", about = "Linux iMessage sync daemon for Omarchy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run sync daemon (foreground)
    Run,
    /// Query local cache status
    Status,
    /// Send one JSON request to the local socket (for Omarchy plugin)
    Request {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("imsg_sync=info".parse()?))
        .init();

    match Cli::parse().command {
        Commands::Run => run_daemon().await,
        Commands::Status => status().await,
        Commands::Request { method, params } => request(&method, &params).await,
    }
}

async fn request(method: &str, params: &str) -> Result<()> {
    let config = config::SyncConfig::load()?;
    let params: serde_json::Value = serde_json::from_str(params)?;
    let req = imsg_proto::Envelope::Req {
        id: "1".into(),
        method: method.into(),
        params,
    };
    let mut stream = tokio::net::UnixStream::connect(&config.socket_path).await?;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    stream
        .write_all(format!("{}\n", req.to_line()?).as_bytes())
        .await?;
    let mut lines = BufReader::new(stream).lines();
    if let Some(line) = lines.next_line().await? {
        println!("{line}");
    }
    Ok(())
}

async fn run_daemon() -> Result<()> {
    let config = config::SyncConfig::load()?;
    let cache = cache::MessageCache::open(&config.cache_path).await?;
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(256);
    let cache = std::sync::Arc::new(tokio::sync::RwLock::new(cache));
    let uplink = uplink::UplinkHandle::default();

    let client_cache = std::sync::Arc::clone(&cache);
    let client_config = config.clone();
    let client_tx = event_tx.clone();
    let client_uplink = uplink.clone();
    tokio::spawn(async move {
        if let Err(e) =
            client::bridge_loop(client_config, client_cache, client_tx, client_uplink).await
        {
            tracing::error!("bridge loop: {e}");
        }
    });

    socket_server::serve(config.socket_path, cache, event_tx, uplink).await
}

async fn status() -> Result<()> {
    let config = config::SyncConfig::load()?;
    let cache = cache::MessageCache::open(&config.cache_path).await?;
    let chats = cache.chat_count().await?;
    let messages = cache.message_count().await?;
    println!("cache: {:?}", config.cache_path);
    println!("chats: {chats}, messages: {messages}");
    println!("bridge: {}", config.bridge_url);
    Ok(())
}
