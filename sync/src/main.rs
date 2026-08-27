use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "imsg-sync", about = "Linux iMessage sync daemon for Omarchy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run,
    Status,
    Request {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    imsg_sync::install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("imsg_sync=info".parse()?))
        .init();

    match Cli::parse().command {
        Commands::Run => imsg_sync::commands::run_daemon().await,
        Commands::Status => {
            let status = imsg_sync::commands::status().await?;
            println!("cache: {:?}", status.cache_path);
            println!("chats: {}, messages: {}", status.chats, status.messages);
            println!("bluebubbles: {}", status.bridge_url);
            Ok(())
        }
        Commands::Request { method, params } => {
            let line = imsg_sync::commands::request(&method, &params).await?;
            println!("{line}");
            Ok(())
        }
    }
}
