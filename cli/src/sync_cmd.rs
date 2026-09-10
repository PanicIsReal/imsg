use crate::SyncCommands;
use anyhow::Result;

pub async fn run(command: SyncCommands, json: bool) -> Result<()> {
    match command {
        SyncCommands::Run => imsg_sync::commands::run_daemon().await,
        SyncCommands::Status => {
            let status = imsg_sync::commands::status().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("cache: {:?}", status.cache_path);
                println!("chats: {}, messages: {}", status.chats, status.messages);
                println!("bridge: {}", status.bridge_url);
            }
            Ok(())
        }
        SyncCommands::Request { method, params } => {
            let line = imsg_sync::commands::request(&method, &params).await?;
            println!("{line}");
            Ok(())
        }
        SyncCommands::Doctor => {
            let report = imsg_sync::commands::doctor()?;
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
            Ok(())
        }
    }
}
