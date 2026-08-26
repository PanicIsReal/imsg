#!/bin/bash
set -euo pipefail
WRAPPER="${HOME}/.local/libexec/imsg-bridge-serve"
APP="/Applications/Ghostty.app"
PORT=18789

PROBE_SRC="$(cd "$(dirname "$0")" && pwd)/contacts-probe/main.swift"
PROBE_BIN="${HOME}/.local/libexec/imsg-contacts-probe"
if [[ ! -x "$PROBE_BIN" && -f "$PROBE_SRC" ]]; then
  mkdir -p "$(dirname "$PROBE_BIN")"
  /usr/bin/swiftc -O -framework Contacts -o "$PROBE_BIN" "$PROBE_SRC" || true
fi

if [[ ! -x "$WRAPPER" ]]; then
  mkdir -p "$(dirname "$WRAPPER")"
  /bin/cp "$(cd "$(dirname "$0")" && pwd)/imsg-bridge-serve.sh" "$WRAPPER"
  chmod +x "$WRAPPER"
fi

serve_up() {
  /usr/sbin/lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1
}

if ! serve_up; then
  /usr/bin/open -na "$APP" --args -e "$WRAPPER"
  sleep 3
fi

if ! serve_up; then
  sleep 30
  exit 1
fi

while serve_up; do
  sleep 5
done
