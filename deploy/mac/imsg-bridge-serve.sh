#!/bin/bash
if [[ ! -f /opt/homebrew/bin/imsg && ! -f /usr/local/bin/imsg ]]; then
  if [[ -x /opt/homebrew/bin/brew ]]; then
    /opt/homebrew/bin/brew install steipete/tap/imsg
  elif [[ -x /usr/local/bin/brew ]]; then
    /usr/local/bin/brew install steipete/tap/imsg
  elif command -v brew >/dev/null 2>&1; then
    brew install steipete/tap/imsg
  else
    echo "brew install steipete/tap/imsg" >&2
  fi
fi
exec "${IMSG:-$HOME/.cargo/bin/imsg}" bridge serve
