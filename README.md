# imsg

iMessage on Omarchy, over Tailscale.

This is the monorepo: Linux sync plus the Omarchy bar plugin.
The Mac side is BlueBubbles Server. Edit `plugin/` here.
`PanicIsReal/omarchy-imessage` is a published copy of that folder so
`omarchy plugin add` can clone a tree with `manifest.json` at the git root.

## Set up

The iMessage panel Settings form is the Linux link. BlueBubbles Server is the Mac app. The BlueBubbles password is stored in the system keyring, never in `config.toml`.

### Mac

1. Install [BlueBubbles Server](https://github.com/BlueBubblesApp/bluebubbles-server/releases/latest). Right-click Open the `.dmg`.
2. Grant Full Disk Access. Set a server password. Leave the proxy off if you use Tailscale.
3. Keep the Mac awake. Default port is 1234.

### Arch Linux

Install `imsg` from the Linux release. Start the local daemon (`imsg sync run` or the systemd unit). Open the iMessage panel and use Settings (the gear) to enter the BlueBubbles URL and password. First run shows that same form. Reconnect from Settings reloads the Mac link without restarting the daemon.

`imsg setup connect` still writes the same store if you prefer the CLI:

```sh
imsg setup connect --url http://<mac-tailscale-ip>:1234 --password <password>
imsg sync run
```

You do not need `bluebubbles-bin` on Linux. The Omarchy plugin talks to local `imsg-sync` only.

## Releases

https://github.com/PanicIsReal/imsg/releases

Each release has two downloads.

- `imsg-macos-aarch64.tar.gz` has `imsg`, `imsg-tui`, and **Imsg Setup.app**.
- `imsg-linux-x86_64.tar.gz` has `imsg` for Omarchy.

## Repo

| Path | What |
|------|------|
| `bridge/` | Mac daemon |
| `sync/` | Linux daemon |
| `plugin/` | Omarchy QML plugin (source of truth) |
| `cli/` | `imsg` binary |
| `setup/` | Pairing wizard |

Agents: see [AGENTS.md](AGENTS.md). After changing `plugin/`, run
`./scripts/publish-plugin.sh` (CI does this on `main`).

## Notes

The bridge refuses `0.0.0.0`. Use a Tailscale or LAN address. Pairing codes last 15 minutes. The Omarchy plugin talks to the local Linux daemon only.
