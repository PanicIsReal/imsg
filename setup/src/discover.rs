use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::time::Duration;

const SERVICE_TYPE: &str = "_imsg-bridge._tcp.local.";

#[derive(Debug, Serialize)]
pub struct DiscoveredBridge {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub bridge_port: String,
    pub enroll_port: String,
}

#[derive(Debug, Serialize)]
pub struct DiscoverResult {
    pub bridges: Vec<DiscoveredBridge>,
}

pub async fn browse(timeout_secs: u64) -> Result<DiscoverResult> {
    let mdns = ServiceDaemon::new()?;
    let receiver = mdns.browse(SERVICE_TYPE)?;

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut bridges = Vec::new();

    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let props = info.get_properties();
                bridges.push(DiscoveredBridge {
                    name: info.get_fullname().to_string(),
                    address: info
                        .get_addresses()
                        .iter()
                        .next()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "?".into()),
                    port: info.get_port(),
                    bridge_port: props
                        .get("bridge_port")
                        .map(|v| v.val_str().to_string())
                        .unwrap_or_else(|| "?".into()),
                    enroll_port: props
                        .get("enroll_port")
                        .map(|v| v.val_str().to_string())
                        .unwrap_or_else(|| "?".into()),
                });
            }
            Ok(ServiceEvent::SearchStarted(_)) | Ok(ServiceEvent::ServiceFound(_, _)) => {}
            Ok(_) => {}
            Err(_) => {}
        }
    }

    Ok(DiscoverResult { bridges })
}

pub async fn browse_human(timeout_secs: u64) -> Result<()> {
    println!("Browsing for {SERVICE_TYPE} ({timeout_secs}s) ...");
    let result = browse(timeout_secs).await?;
    if result.bridges.is_empty() {
        println!("No bridges found.");
        println!("Tip: enable mDNS on Mac with `imsg bridge init --mdns` or use Tailscale MagicDNS hostname.");
    } else {
        for b in &result.bridges {
            println!(
                "- {} @ {}:{} (bridge_port={}, enroll_port={})",
                b.name, b.address, b.port, b.bridge_port, b.enroll_port
            );
        }
        println!(
            "Found {} bridge(s). Pair with: imsg setup pair <code> --host <hostname>",
            result.bridges.len()
        );
    }
    Ok(())
}
