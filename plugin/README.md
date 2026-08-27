# iMessage

iMessage inbox and composer for the Omarchy bar.

This plugin talks only to a local `imsg-sync` daemon on a Unix socket. It never opens a network connection to a Mac.

Edit the plugin in the [imsg monorepo](https://github.com/PanicIsReal/imsg) `plugin/` directory. This GitHub repo is a published copy of that folder: `omarchy plugin add` requires `manifest.json` at the clone root, so it cannot point at the monorepo.

## Prerequisites

The Mac must have Homebrew `imsg` before the bridge can read Messages.

```sh
brew install steipete/tap/imsg
```

`imsg-bridge serve` installs that formula when Homebrew is already present. If Homebrew is missing, install it from https://brew.sh, then run the command above.

The Mac bridge also needs Full Disk Access for Ghostty so it can read Messages. The plugin never asks for that permission itself.

## Install

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
```

Chats stay empty until `imsg-sync` runs on this machine. Install the companion CLI from https://github.com/PanicIsReal/imsg. Pair the Mac bridge. Start `imsg-sync`. Then log out and back in so the bar picks up the widget.

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
