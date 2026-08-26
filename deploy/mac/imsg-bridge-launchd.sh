#!/bin/bash
set -euo pipefail
BRIDGE="/Users/panic/.cargo/bin/imsg"
APP="/Applications/Ghostty.app"

# Status-only TCC probe. Reading CNContactStore.authorizationStatus does not prompt.
PROBE_SRC="$(cd "$(dirname "$0")" && pwd)/contacts-probe/main.swift"
PROBE_BIN="${HOME}/.local/libexec/imsg-contacts-probe"
if [[ ! -x "$PROBE_BIN" && -f "$PROBE_SRC" ]]; then
  mkdir -p "$(dirname "$PROBE_BIN")"
  /usr/bin/swiftc -O -framework Contacts -o "$PROBE_BIN" "$PROBE_SRC" || true
fi

if ! /usr/bin/pgrep -fq "$BRIDGE bridge serve"; then
  /usr/bin/open -na "$APP" --args -e "$BRIDGE bridge serve"
  sleep 3
fi

while /usr/bin/pgrep -fq "$BRIDGE bridge serve"; do
  sleep 5
done
