#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bin="${IMSG_BIN:-$root/target/release/imsg}"
if [[ ! -x "$bin" && -x "$root/target/x86_64-unknown-linux-gnu/release/imsg" ]]; then
  bin="$root/target/x86_64-unknown-linux-gnu/release/imsg"
fi
if [[ ! -x "$bin" ]]; then
  echo "missing imsg binary at $bin; build with: cargo build --release -p imsg-cli" >&2
  exit 1
fi

mkdir -p "$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp "$bin" "$stage/imsg"
chmod 755 "$stage/imsg"

out="$root/dist/imsg-linux-x86_64.tar.gz"
tar -C "$stage" -czf "$out" imsg

if command -v shasum >/dev/null; then
  (cd "$root/dist" && shasum -a 256 imsg-linux-x86_64.tar.gz > imsg-linux-x86_64.tar.gz.sha256)
else
  (cd "$root/dist" && sha256sum imsg-linux-x86_64.tar.gz > imsg-linux-x86_64.tar.gz.sha256)
fi

echo "$out"
