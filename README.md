# imsg — Omarchy iMessage Plugin

Tailscale/LAN-only iMessage bridge for [Omarchy](https://omarchy.org). Read-only by default.

## Architecture

| Component | Platform | Role |
|-----------|----------|------|
| `imsg-bridge` | macOS | Wraps `imsg rpc`, serves WSS + mTLS |
| `imsg-sync` | Linux | Caches messages locally, Unix socket API |
| `omarchy-imessage` | Linux | Omarchy bar widget + chat panel (QML) |

## Prerequisites

### Mac

```bash
brew install steipete/tap/imsg
cargo install --path bridge
```

Grant **Full Disk Access** to `imsg-bridge` and `imsg` in System Settings → Privacy & Security.

Optional: **Contacts** for name resolution.

### Linux (Omarchy)

```bash
cargo install --path sync
```

## Setup

### 1. Initialize bridge (Mac)

```bash
imsg-bridge init --bind 127.0.0.1   # use Tailscale IP in production
imsg-bridge pair                    # shows pairing code + CA path
```

Import client cert after generating on Linux:

```bash
imsg-bridge clients-import omarchy-laptop --cert ~/client.pem
```

### 2. Run bridge

```bash
imsg-bridge serve
# or: launchctl load deploy/mac/imsg-bridge.plist
```

### 3. Configure sync (Linux)

Create `~/.config/imsg-sync/config.toml`:

```toml
bridge_url = "wss://100.x.x.x:18789/ws"
ca_cert_path = "/home/you/.local/share/omarchy-imessage/ca.pem"
client_cert_path = "/home/you/.local/share/omarchy-imessage/client.pem"
client_key_path = "/home/you/.local/share/omarchy-imessage/client-key.pem"
```

```bash
imsg-sync run
# or: systemctl --user enable --now deploy/linux/imsg-sync.service
```

### 4. Install Omarchy plugin

```bash
omarchy plugin add /path/to/imsg/plugin --enable
# or copy plugin/ to ~/.config/omarchy/plugins/io.github.panic.imessage/
omarchy-shell shell rescanPlugins
```

## Security

- Bridge binds to Tailscale/LAN only (refuses `0.0.0.0`)
- mTLS + client cert allowlist
- Read-only API allowlist (no `send` in v1)
- Plugin talks to local Unix socket only
- See `docs/tailscale-acl.json` for example ACLs

## Development

```bash
cargo test
./scripts/pr0-prototype.sh
```

## License

MIT
