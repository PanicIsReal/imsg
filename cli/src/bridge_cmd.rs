use crate::BridgeCommands;
use anyhow::Result;

pub async fn run(command: crate::BridgeCommands, json: bool) -> Result<()> {
    match command {
        BridgeCommands::Init { bind, port, mdns } => {
            let result = imsg_bridge::commands::init(bind, port, mdns)?;
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
        BridgeCommands::Pair { rotate } => {
            if let Some(v) = imsg_bridge::commands::pair(json, rotate)? {
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        BridgeCommands::ClientsImport { name, cert } => {
            let path = imsg_bridge::commands::import_client(name, cert)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "path": path }))?);
            } else {
                println!("Imported client cert to {:?}", path);
            }
        }
        BridgeCommands::Serve => imsg_bridge::commands::serve().await?,
        BridgeCommands::Status => {
            let status = imsg_bridge::commands::status().await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        BridgeCommands::Doctor => {
            let report = imsg_bridge::commands::doctor()?;
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
