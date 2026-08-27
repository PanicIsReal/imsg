# iMessage

iMessage inbox and composer for the Omarchy bar.

This plugin talks only to a local `imsg-sync` daemon on a Unix socket. It never opens a network connection to a Mac.

Edit the plugin in the [imsg monorepo](https://github.com/PanicIsReal/imsg) `plugin/` directory. This GitHub repo is a published copy of that folder: `omarchy plugin add` requires `manifest.json` at the clone root, so it cannot point at the monorepo.

## Prerequisites

Run [BlueBubbles Server](https://github.com/BlueBubblesApp/bluebubbles-server/releases/latest) on a Mac signed into iMessage. Grant it Full Disk Access. You do not need `bluebubbles-bin` on Linux. The plugin talks only to local `imsg-sync`.

```sh
imsg setup connect --url http://<mac-tailscale-ip>:1234 --password <bluebubbles-password>
imsg sync run
```

## Install

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
```

Chats stay empty until `imsg-sync` can reach BlueBubbles. Then log out and back in so the bar picks up the widget.

## Usage

Click the bar icon to open the panel. The first run shows a setup card until a local cache exists. After that, select a conversation to read it. Type a reply and press Enter or Send when the Mac link is live.

The bar icon shows the unread count once conversations are cached.

## Configure

```sh
omarchy bar move io.github.panic.imessage --section right
```

## Remove

```sh
omarchy plugin remove io.github.panic.imessage
```

This command deletes the plugin files and the bar entry. It does not stop `imsg-sync` or delete the local message cache.
