# imsg

iMessage on Omarchy, over Tailscale.

## Set up

`imsg setup` is the product. Download a release, then run that.

### Mac

1. Install [Ghostty](https://ghostty.org) and grant it Full Disk Access.
2. Download `imsg-macos-aarch64.tar.gz` from [GitHub Releases](https://github.com/PanicIsReal/imsg/releases).
3. Open **Imsg Setup**, or run `imsg setup` in Ghostty.
4. Work the checklist. The pairing code stays on screen after enroll is up.
5. When asked, connect Omarchy over SSH. The default host is `omarchy` if that SSH alias exists.

The wizard installs Homebrew `steipete/tap/imsg` and the Mac LaunchAgent. It can install Linux `imsg` over SSH, pair, and enable the plugin. The client private key is created on Linux.

### Arch Linux

If you skip SSH from the Mac, install the Linux binary from the same release. `packaging/arch/PKGBUILD` installs it to `/usr/bin/imsg`.

Pairing still starts on the Mac.

## Releases

https://github.com/PanicIsReal/imsg/releases

- `imsg-macos-aarch64.tar.gz` — `imsg`, `imsg-tui`, and `Imsg Setup.app`
- `imsg-linux-x86_64.tar.gz` — `imsg` for Omarchy

## Notes

The bridge refuses `0.0.0.0`. Use a Tailscale or LAN address. Pairing codes last 15 minutes. The Omarchy plugin talks to the local Linux daemon only.
