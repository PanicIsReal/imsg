#!/bin/bash
set -euo pipefail
BRIDGE="/Users/panic/.cargo/bin/imsg"
APP="/Applications/Ghostty.app"

if ! /usr/bin/pgrep -fq "$BRIDGE bridge serve"; then
  /usr/bin/open -na "$APP" --args -e "$BRIDGE bridge serve"
  sleep 3
fi

while /usr/bin/pgrep -fq "$BRIDGE bridge serve"; do
  sleep 5
done
