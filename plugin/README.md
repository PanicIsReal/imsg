# iMessage

iMessage inbox and composer for the Omarchy bar.

## Install

```sh
omarchy plugin add https://github.com/PanicIsReal/omarchy-imessage.git --enable
```

This plugin talks only to a local `imsg-sync` daemon on a Unix socket. It never opens a network connection to a Mac.

Chats stay empty until that daemon runs on this machine. Install the companion CLI from https://github.com/PanicIsReal/imsg. Pair the Mac bridge. Start `imsg-sync`. Then log out and back in so the bar picks up the widget.

The Mac bridge needs Full Disk Access for Ghostty so it can read Messages. The plugin never asks for that permission itself.

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
