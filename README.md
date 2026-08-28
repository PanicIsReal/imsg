# imsg

iMessage on Omarchy. BlueBubbles runs on a Mac signed into iMessage.
`imsg-sync` runs on Linux and talks to that server. The bar plugin talks
only to the local daemon over a Unix socket.

## Install

### Run BlueBubbles on the Mac

1. Install [BlueBubbles Server](https://github.com/BlueBubblesApp/bluebubbles-server/releases/latest). Right-click the `.dmg` and choose Open.
2. Grant Full Disk Access to BlueBubbles in System Settings, Privacy and Security.
3. Set a server password in BlueBubbles.
4. If you use Tailscale, leave the BlueBubbles proxy off.
5. Keep the Mac awake. The default listen port is 1234.

HTTP on the Tailscale IP is enough:

```sh
http://<mac-tailscale-ip>:1234
```

For HTTPS on MagicDNS, on the Mac run:

```sh
tailscale serve --bg 1234
```

The URL is then `https://<machine>.<tailnet>.ts.net`. If Serve uses another
HTTPS port, put that port on the URL.

Do not install `bluebubbles-bin` on Linux.

### Build the Linux daemon

GitHub Releases `v0.1.0` is the old Mac pairing build. Build `imsg` from this repo.

```sh
git clone https://github.com/PanicIsReal/imsg.git
cd imsg
cargo install --path cli --locked
imsg install --plugin plugin/
```

`imsg install` writes `~/.config/systemd/user/imsg-sync.service` with the
real binary path, copies the plugin, and starts the daemon.

To take the published plugin instead of `plugin/`, after `cargo install`:

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
imsg install
```

`omarchy plugin add` requires `manifest.json` at the clone root. Point it at
`PanicIsReal/omarchy-imessage`, not this monorepo.

### Enter the URL and password

Open the iMessage panel. The first run is Settings. Enter the BlueBubbles URL
and password. Save. The password is stored in the system keyring as service
`imsg-sync` and username `bluebubbles`. It is never written to `config.toml`.

The CLI writes the same store:

```sh
imsg setup connect --url http://<mac-tailscale-ip>:1234 --password <password>
```

Prefer the panel form so the password does not land in shell history.
Reconnect from Settings retries the Mac without restarting the daemon.

### Optional webhook

New iMessages default to a 2s REST poll. Settings can switch to a webhook doorbell.
The POST is only a poke. imsg-sync then pulls the real message over REST with the
password. Poll is off while the webhook is on.

1. In Settings, set the Serve URL (MagicDNS HTTPS origin for this Linux box).
2. Pick a port if 18792 is taken. Enable webhook.
3. On Linux, click **Publish with Tailscale** in Settings. That opens the
same floating Omarchy window as install, and runs `tailscale serve` for the
port in the form. Do not use Funnel. To do it by hand:

```sh
tailscale serve --bg localhost:18792
```

4. Copy the webhook URL. Treat the token like a password.
5. Connect to BlueBubbles, then click Register webhook.

Restrict the Linux Serve ACL to your Mac.

## Use

Click the bar icon, or Super+Ctrl+I, to open the panel. j/k move conversations.
l or Enter focuses the composer. Escape blurs. Escape again closes. Enter sends.
a or Photo attaches an image. A send shows as Sending until the Mac accepts it.

Add the keybind in `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + CTRL + I", "iMessage", "omarchy-shell shell toggle io.github.panic.imessage")
```

Move the widget with:

```sh
omarchy bar move io.github.panic.imessage --section right
```

## Remove

```sh
omarchy plugin remove io.github.panic.imessage
imsg uninstall
```

`imsg uninstall --purge` also deletes the local cache and config. It does not
stop BlueBubbles on the Mac.

## Repo

| Path | What |
|------|------|
| `sync/` | Linux daemon. BlueBubbles REST, cache, Unix socket |
| `plugin/` | Omarchy QML plugin. Source of truth |
| `cli/` | `imsg` binary |
| `bridge/` | Leftover Mac daemon. Do not add features |
| `setup/` | Leftover pairing wizard |
| `tui/` | Leftover Mac setup TUI |

See [AGENTS.md](AGENTS.md) for agent rules. After you change `plugin/`, run
`./scripts/publish-plugin.sh`. CI does that on `main` when
`PLUGIN_SYNC_SSH_KEY` is set.
