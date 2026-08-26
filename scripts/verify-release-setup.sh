#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
fail=0

need() {
  if [[ ! -e "$1" ]]; then
    echo "missing $1"
    fail=1
  fi
}

need .github/workflows/release.yml
need packaging/arch/PKGBUILD
need scripts/codesign-notarize.sh
need scripts/package-mac.sh
need scripts/package-linux.sh
need tui/src/App.tsx

if ! grep -q "kind: 'ask-ssh'" tui/src/App.tsx && ! grep -q 'kind: "ask-ssh"' tui/src/App.tsx; then
  echo "tui/src/App.tsx has no ask-ssh screen"
  fail=1
fi

if ! grep -qE 'Push \{' cli/src/main.rs; then
  echo "cli/src/main.rs has no setup push command"
  fail=1
fi

if [[ -f scripts/codesign-notarize.sh ]]; then
  if ! APPLE_SKIP=1 bash scripts/codesign-notarize.sh --dry-run >/tmp/imsg-notary-dry.txt 2>&1; then
    echo "codesign dry-run failed"
    cat /tmp/imsg-notary-dry.txt
    fail=1
  fi
fi

if command -v cargo >/dev/null; then
  cargo test -p imsg-setup --offline -- --test-threads=1 || fail=1
fi

if command -v bun >/dev/null; then
  (cd tui && bunx tsc --noEmit) || fail=1
fi

if [[ "$fail" -ne 0 ]]; then
  echo "verify-release-setup failed"
  exit 1
fi

echo "verify-release-setup ok"
