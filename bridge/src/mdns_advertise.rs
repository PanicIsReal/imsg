use crate::config::Config;
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::{info, warn};

const SERVICE_TYPE: &str = "_imsg-bridge._tcp.local.";

/// Advertise bridge presence on local LAN via mDNS (opt-in).
/// Does not expose pairing codes or credentials — discovery only.
pub fn spawn_if_enabled(config: &Config) -> Result<()> {
    if !config.mdns_advertise {
        return Ok(());
    }

    let config = config.clone();
    std::thread::spawn(move || {
        if let Err(e) = advertise(&config) {
            warn!("mDNS advertise stopped: {e}");
        }
    });
    Ok(())
}

fn advertise(config: &Config) -> Result<()> {
    let mdns = ServiceDaemon::new()?;
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "imsg-bridge".into());
    let instance = format!("{host}.");
    let host_ipv4 = if config.bind.contains(':') {
        format!("{}.local.", host)
    } else {
        format!("{}.local.", host)
    };

    let properties = [
        ("version", "1"),
        ("proto", "imsg-bridge/v1"),
        ("bridge_port", &config.port.to_string()),
        ("enroll_port", &config.enroll_port.to_string()),
    ];

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_ipv4,
        &config.bind,
        config.port,
        &properties[..],
    )?;
    mdns.register(service)?;
    info!(
        "mDNS advertising {} on {}:{} (enroll {})",
        SERVICE_TYPE, config.bind, config.port, config.enroll_port
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
