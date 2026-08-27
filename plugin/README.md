# iMessage

iMessage inbox and composer for the Omarchy bar.

This plugin talks only to a local `imsg-sync` daemon on a Unix socket. It never
opens a network connection to a Mac.

Edit the plugin in the [imsg monorepo](https://github.com/PanicIsReal/imsg)
`plugin/` directory. This GitHub repo is a published copy of that folder.
`omarchy plugin add` requires `manifest.json` at the clone root, so it cannot
point at the monorepo.

## Prerequisites

Run [BlueBubbles Server](https://github.com/BlueBubblesApp/bluebubbles-server/releases/latest)
on a Mac signed into iMessage. Grant it Full Disk Access. Set a server password.

On Linux, build and start `imsg-sync` from the [imsg install guide](https://github.com/PanicIsReal/imsg#install).
You do not need `bluebubbles-bin` on Linux.

## Install

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
```

From a clone of the monorepo, `imsg install --plugin plugin/` copies this
folder and starts the daemon.

## Link

Open the panel. The first run is Settings. Enter the BlueBubbles URL and
password. Save. The password is stored in the system keyring, not in a config
file. Reconnect from Settings retries the Mac without restarting sync.

The CLI writes the same store:

```sh
imsg setup connect --url http://<mac-tailscale-ip>:1234 --password <password>
```

Prefer the panel form so the password does not land in shell history.

## Use

Click the bar icon, or Super+Ctrl+I, to open the panel. j/k move conversations.
l or Enter focuses the composer. Escape blurs. Escape again closes. Enter sends.
a or Photo attaches an image.

Add the keybind in `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + CTRL + I", "iMessage", "omarchy-shell shell toggle io.github.panic.imessage")
```

The gear opens Settings later. The bar icon shows the unread count once
conversations are cached.

## Configure

```sh
omarchy bar move io.github.panic.imessage --section right
```

## Remove

```sh
omarchy plugin remove io.github.panic.imessage
```

This command deletes the plugin files and the bar entry. It does not stop
`imsg-sync` or delete the local message cache.
