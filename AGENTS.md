# imsg

Monorepo for iMessage on Omarchy: Mac bridge, Linux sync, and the bar plugin.

Work in this clone. Do not open `PanicIsReal/omarchy-imessage` to edit QML.
That repo is a published copy of `plugin/` so `omarchy plugin add` can clone a
tree with `manifest.json` at the git root.

## Layout

| Path | What |
|------|------|
| `bridge/` | macOS daemon. Wraps `imsg rpc`, mTLS WebSocket, pairing. |
| `sync/` | Linux daemon. Cache, uplink to the Mac, Unix socket for the plugin. |
| `proto/` | Shared wire types and `proto/v1.md`. |
| `setup/` | Pairing / doctor / SSH push (`imsg setup`). |
| `cli/` | The `imsg` binary. |
| `plugin/` | Omarchy QML/JS plugin. Source of truth. |
| `tui/` | Mac setup TUI (bun). |
| `deploy/` | LaunchAgent and systemd units. |
| `packaging/` | Arch PKGBUILD and Mac app wrapper. |

Rust workspace members are `bridge`, `sync`, `proto`, `setup`, `cli`. The
plugin is QML. Do not add Rust inside `plugin/` or network calls from QML.

## Boundaries

- The plugin talks only to the local `imsg-sync` Unix socket.
- `imsg-sync` is the only Linux process that talks to the Mac.
- `imsg-bridge` is the only process that reads Messages / `chat.db`.
- `proto/v1.md` is the wire contract. Change it before the crates.

## Commands

```sh
cargo test
cargo build -p imsg-cli
./scripts/validate-plugin.sh
omarchy plugin validate plugin/   # if omarchy is on PATH
```

After editing `plugin/`, publish the installable slice:

```sh
./scripts/publish-plugin.sh
```

CI also publishes `plugin/` to `PanicIsReal/omarchy-imessage` on push to
`main` when `PLUGIN_SYNC_SSH_KEY` is set.

## CI runners

All GitHub Actions jobs use self-hosted runners. None of them should use
`ubuntu-latest` or `macos-latest`.

| Jobs | `runs-on` |
|------|-----------|
| `plugin`, `linux`, `publish`, `publish-plugin` | `[self-hosted, linux]` |
| `mac` (build, TUI, codesign) | `[self-hosted, macOS]` |

GitHub applies those OS labels automatically when you register a runner.
This repo currently has zero runners, so jobs will queue until both exist.

Register from the machine that should run the jobs:

1. Repo **Settings → Actions → Runners → New self-hosted runner**
2. Follow the Linux x64 or macOS ARM64 instructions
3. `./config.sh --url https://github.com/PanicIsReal/imsg --token <token>`
4. Install as a service (`./svc.sh install && ./svc.sh start` on Linux;
   the Mac equivalent in the GitHub script)

Linux runner needs `git`, `jq`, `file`, `tar`, and network (the workflow
installs the Rust toolchain). Mac runner needs Xcode CLT, `git`, and
network (`setup-bun` plus rustup). Codesign uses the Mac keychain or the
existing `DEVELOPER_ID_*` / `NOTARY_*` secrets.

## Install URLs (do not "fix" these)

- Releases and this repo: `https://github.com/PanicIsReal/imsg`
- `omarchy plugin add`: `https://github.com/PanicIsReal/omarchy-imessage.git`

`setup/src/push.rs` `DEFAULT_PLUGIN_REPO` must stay the satellite URL.
`omarchy-plugin-add` clones the URL and validates `manifest.json` at the
clone root, so pointing it at this monorepo would install the whole Rust
tree into `~/.config/omarchy/plugins/`.
