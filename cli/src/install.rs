use crate::info::{self, Role};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct InstallResult {
    pub role: Role,
    pub service: String,
    pub plugin_installed: bool,
}

pub async fn install(plugin: Option<PathBuf>, json: bool) -> Result<()> {
    let platform = info::detect();
    let imsg = info::imsg_path()?;
    let mut plugin_installed = false;

    let service = match platform.role {
        Role::Mac => {
            install_mac_launchagent(&imsg)?;
            "launchagent: com.panic.imsg-bridge".into()
        }
        Role::Linux => {
            install_linux_systemd(&imsg)?;
            if let Some(plugin_path) = plugin.or_else(default_plugin_path) {
                install_plugin(&plugin_path)?;
                plugin_installed = true;
            }
            "systemd: imsg-sync.service".into()
        }
        Role::Unknown => anyhow::bail!("unsupported platform: {}", platform.os),
    };

    let result = InstallResult {
        role: platform.role,
        service,
        plugin_installed,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Installed {}", result.service);
        if plugin_installed {
            println!("Plugin installed to {:?}", platform.config_paths.plugin_dir);
        }
        println!("Start with: imsg {} run", if platform.role == Role::Mac { "bridge serve" } else { "sync" });
    }
    Ok(())
}

pub async fn uninstall(purge: bool, json: bool) -> Result<()> {
    let platform = info::detect();
    match platform.role {
        Role::Mac => uninstall_mac_launchagent()?,
        Role::Linux => {
            uninstall_linux_systemd()?;
            if platform.config_paths.plugin_dir.exists() {
                fs::remove_dir_all(&platform.config_paths.plugin_dir)?;
            }
        }
        Role::Unknown => anyhow::bail!("unsupported platform"),
    }

    if purge {
        let _ = fs::remove_dir_all(
            platform
                .config_paths
                .bridge_config
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        );
        let _ = fs::remove_dir_all(&platform.config_paths.sync_state);
        let _ = fs::remove_dir_all(
            platform
                .config_paths
                .sync_config
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        );
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "ok": true, "purged": purge }))?);
    } else {
        println!("Uninstalled services{}", if purge { " and removed data" } else { "" });
    }
    Ok(())
}

fn install_mac_launchagent(imsg: &PathBuf) -> Result<()> {
    let home = dirs::home_dir().context("home dir")?;
    let support = home.join("Library/Application Support/imsg-bridge");
    let wrapper = home.join(".local/libexec/imsg-bridge-serve");
    let launchd_sh = support.join("launchd.sh");
    let plist_path = home.join("Library/LaunchAgents/com.panic.imsg-bridge.plist");

    fs::create_dir_all(support)?;
    if let Some(parent) = wrapper.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &wrapper,
        format!("#!/bin/bash\nexec \"{}\" bridge serve\n", imsg.display()),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    }

    fs::write(
        &launchd_sh,
        format!(
            r#"#!/bin/bash
set -euo pipefail
WRAPPER="{}"
APP="/Applications/Ghostty.app"
PORT=18789

serve_up() {{
  /usr/sbin/lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1
}}

if ! serve_up; then
  /usr/bin/open -na "$APP" --args -e "$WRAPPER"
  sleep 3
fi

if ! serve_up; then
  sleep 30
  exit 1
fi

while serve_up; do
  sleep 5
done
"#,
            wrapper.display()
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launchd_sh, fs::Permissions::from_mode(0o755))?;
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.panic.imsg-bridge</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/imsg-bridge.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/imsg-bridge.err.log</string>
</dict>
</plist>
"#,
        launchd_sh.display()
    );

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&plist_path, plist)?;
    Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&plist_path)
        .status()
        .context("launchctl load")?;
    Ok(())
}

fn uninstall_mac_launchagent() -> Result<()> {
    let home = dirs::home_dir().context("home dir")?;
    let plist_path = home.join("Library/LaunchAgents/com.panic.imsg-bridge.plist");
    let _ = Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(&plist_path)
        .status();
    let _ = fs::remove_file(plist_path);
    Ok(())
}

fn install_linux_systemd(imsg: &PathBuf) -> Result<()> {
    let home = dirs::home_dir().context("home dir")?;
    let unit_dir = home.join(".config/systemd/user");
    fs::create_dir_all(&unit_dir)?;
    let unit_path = unit_dir.join("imsg-sync.service");
    let unit = format!(
        r#"[Unit]
Description=iMessage sync daemon for Omarchy
After=network-online.target tailscaled.service
Wants=network-online.target

[Service]
Type=simple
ExecStart={} sync run
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=imsg_sync=info

[Install]
WantedBy=default.target
"#,
        imsg.display()
    );
    fs::write(&unit_path, unit)?;
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("systemctl daemon-reload")?;
    Command::new("systemctl")
        .args(["--user", "enable", "--now", "imsg-sync.service"])
        .status()
        .context("systemctl enable imsg-sync")?;
    Ok(())
}

fn uninstall_linux_systemd() -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "imsg-sync.service"])
        .status();
    let home = dirs::home_dir().context("home dir")?;
    let unit_path = home.join(".config/systemd/user/imsg-sync.service");
    let _ = fs::remove_file(unit_path);
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(())
}

fn default_plugin_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let candidate = cwd.join("plugin");
    if candidate.join("manifest.json").exists() {
        Some(candidate)
    } else {
        None
    }
}

fn install_plugin(src: &PathBuf) -> Result<()> {
    let dest = info::detect().config_paths.plugin_dir;
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    copy_dir_recursive(src, &dest)?;
    let home = dirs::home_dir().context("home dir")?;
    let local_bin = home.join(".local/bin");
    fs::create_dir_all(&local_bin)?;
    let imsg_src = info::imsg_path().unwrap_or_else(|_| PathBuf::from("imsg"));
    let imsg_link = local_bin.join("imsg");
    if imsg_link.exists() {
        let _ = fs::remove_file(&imsg_link);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = symlink(&imsg_src, &imsg_link);
    }
    let omarchy_path = std::env::var("OMARCHY_PATH").unwrap_or_else(|_| "/usr/share/omarchy".into());
    let _ = Command::new("omarchy")
        .env("OMARCHY_PATH", &omarchy_path)
        .args(["plugin", "enable", "io.github.panic.imessage", "right"])
        .status();
    let _ = Command::new("omarchy-shell")
        .env("OMARCHY_PATH", &omarchy_path)
        .args(["shell", "rescanPlugins"])
        .status();
    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dest: &PathBuf) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
