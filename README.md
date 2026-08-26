# imsg

Tailscale or LAN iMessage bridge for [Omarchy](https://omarchy.org).

The bar plugin is published at https://github.com/PanicIsReal/omarchy-imessage. This repository is the Mac bridge and the Linux sync daemon that plugin talks to.

## Architecture

| Component | Platform | Role |
|-----------|----------|------|
| `imsg-bridge` | macOS | Wraps `imsg rpc`, serves WSS + mTLS |
| `imsg-sync` | Linux | Caches messages locally, Unix socket API |
| `omarchy-imessage` | Linux | Omarchy bar widget + chat panel (QML) |

## Prerequisites

### Mac

```sh
brew install steipete/tap/imsg
cargo install --path bridge
```

`imsg-bridge serve` runs that brew install when Homebrew is present and the formula is missing.

Grant Full Disk Access to Ghostty, then run the bridge from that terminal.

Optional: Contacts access so chats show names.

### Linux (Omarchy)

```sh
cargo install --path sync
```

## Setup

### 1. Initialize the bridge on the Mac

```sh
imsg-bridge init --bind 127.0.0.1
imsg-bridge pair
```

Use a Tailscale IP in production, not loopback.

Import the Linux client cert after you generate it:

```sh
imsg-bridge clients-import omarchy-laptop --cert ~/client.pem
```

### 2. Run the bridge

```sh
imsg-bridge serve
```

Or load `deploy/mac/imsg-bridge.plist` with `launchctl`.

### 3. Configure sync on Linux

Create `~/.config/imsg-sync/config.toml`:

```toml
bridge_url = "wss://100.x.x.x:18789/ws"
ca_cert_path = "/home/you/.local/share/omarchy-imessage/ca.pem"
client_cert_path = "/home/you/.local/share/omarchy-imessage/client.pem"
client_key_path = "/home/you/.local/share/omarchy-imessage/client-key.pem"
```

```sh
imsg-sync run
```

Or enable `deploy/linux/imsg-sync.service` with systemd.

### 4. Install the Omarchy plugin

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
```

You can also copy `plugin/` from this repository to `~/.config/omarchy/plugins/io.github.panic.imessage/`.

## Security

- The bridge binds to Tailscale or LAN only. It refuses `0.0.0.0`.
- Clients use mTLS and a cert allowlist.
- The plugin talks to a local Unix socket only. It never connects to the Mac.
- See `docs/tailscale-acl.json` for example ACLs.

## Development

```sh
cargo test
./scripts/pr0-prototype.sh
```

## License

MIT
