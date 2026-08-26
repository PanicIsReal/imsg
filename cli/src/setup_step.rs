use anyhow::Result;
use imsg_setup::step::{refuse_wildcard_bind, wait_enroll, StepId};
use serde_json::json;
use std::time::Duration;

use crate::info;
use crate::install;

pub fn run(id: StepId, bind: Option<String>, mdns: bool, json: bool) -> Result<()> {
    let value = match id {
        StepId::Detect => {
            let platform = info::detect();
            let bind = info::tailscale_ip_hint();
            let ghostty = std::path::Path::new("/Applications/Ghostty.app").exists();
            let detail = match &bind {
                Some(ip) => format!("{}/{} · tailscale {ip}", platform.os, platform.arch),
                None => format!("{}/{}", platform.os, platform.arch),
            };
            json!({
                "id": id.as_str(),
                "ok": true,
                "os": platform.os,
                "arch": platform.arch,
                "role": platform.role,
                "hostname": platform.hostname,
                "bind": bind,
                "ghostty": ghostty,
                "detail": detail,
            })
        }
        StepId::Brew => {
            let path = imsg_bridge::steipete::ensure_steipete_imsg("imsg")?;
            json!({
                "id": id.as_str(),
                "ok": true,
                "detail": path.display().to_string(),
            })
        }
        StepId::Certs => {
            let bind = bind
                .or_else(info::tailscale_ip_hint)
                .or_else(|| {
                    imsg_bridge::config::Config::load()
                        .ok()
                        .map(|c| c.bind)
                        .filter(|b| refuse_wildcard_bind(b).is_ok())
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("no Tailscale or LAN bind; pass --bind <ip>")
                })?;
            refuse_wildcard_bind(&bind)?;
            let result = imsg_bridge::commands::init(bind.clone(), 18789, mdns)?;
            let expires = imsg_bridge::config::Config::load()
                .ok()
                .and_then(|c| c.pairing_code_expires_at);
            json!({
                "id": id.as_str(),
                "ok": true,
                "bind": bind,
                "pairing_code": result.pairing_code,
                "expires_at": expires,
                "enroll_port": result.enroll_port,
                "detail": format!("certs ready, enroll {}", result.enroll_port),
            })
        }
        StepId::Service => {
            let result = install::apply(None)?;
            json!({
                "id": id.as_str(),
                "ok": true,
                "service": result.service,
                "detail": result.service,
            })
        }
        StepId::Enroll => {
            let bind = bind.unwrap_or_else(|| {
                imsg_bridge::config::Config::load()
                    .map(|c| c.bind)
                    .unwrap_or_else(|_| "127.0.0.1".into())
            });
            let port = imsg_bridge::config::Config::load()
                .map(|c| c.enroll_port)
                .unwrap_or(18790);
            wait_enroll(&bind, port, Duration::from_secs(90))?;
            let status = imsg_bridge::commands::pair(true, false)?
                .unwrap_or_else(|| json!({}));
            json!({
                "id": id.as_str(),
                "ok": true,
                "bind": bind,
                "enroll_port": port,
                "pairing_code": status.get("code").cloned().unwrap_or(json!(null)),
                "expires_at": status.get("expires_at").cloned().unwrap_or(json!(null)),
                "enroll_url": status.get("enroll_url").cloned().unwrap_or(json!(null)),
                "detail": format!("enroll listening on {bind}:{port}"),
            })
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}: {}", id.as_str(), value["detail"].as_str().unwrap_or("ok"));
    }
    Ok(())
}
