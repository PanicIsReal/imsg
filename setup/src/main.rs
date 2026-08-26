use anyhow::Result;
use clap::Parser;
use imsg_setup::{discover, doctor, enroll};

#[derive(Parser)]
#[command(name = "imsg-setup", about = "Deprecated: use `imsg setup`")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
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
        #[arg(long)]
        json: bool,
    },
    Discover {
        #[arg(long, default_value_t = 5)]
        timeout: u64,
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("note: imsg-setup is deprecated; use `imsg setup` instead");
    match Cli::parse().command {
        Commands::Pair {
            code,
            host,
            enroll_port,
            name,
            insecure,
            json,
        } => {
            if json {
                let result =
                    enroll::pair(&code, &host, enroll_port, name.as_deref(), insecure).await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                enroll::pair_human(&code, &host, enroll_port, name.as_deref(), insecure).await?;
            }
            Ok(())
        }
        Commands::Discover { timeout, json } => {
            if json {
                let found = discover::browse(timeout).await?;
                println!("{}", serde_json::to_string_pretty(&found)?);
            } else {
                discover::browse_human(timeout).await?;
            }
            Ok(())
        }
        Commands::Doctor { json } => {
            if json {
                let report = doctor::run()?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                doctor::run_human()?;
            }
            Ok(())
        }
    }
}
