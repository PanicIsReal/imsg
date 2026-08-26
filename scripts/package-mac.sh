#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bin="${IMSG_BIN:-$root/target/release/imsg}"
if [[ ! -x "$bin" && -x "$root/target/aarch64-apple-darwin/release/imsg" ]]; then
  bin="$root/target/aarch64-apple-darwin/release/imsg"
fi
if [[ ! -x "$bin" ]]; then
  echo "missing imsg binary at $bin; build with: cargo build --release -p imsg-cli" >&2
  exit 1
fi

tui="${IMSG_TUI_BIN:-$root/tui/imsg-tui}"
if [[ ! -x "$tui" ]]; then
  echo "missing imsg-tui at $tui; compile with: (cd tui && bun run compile)" >&2
  exit 1
fi

mkdir -p "$root/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
pkg="$stage/imsg-macos-aarch64"
mkdir -p "$pkg"

cp "$bin" "$pkg/imsg"
cp "$tui" "$pkg/imsg-tui"
chmod 755 "$pkg/imsg" "$pkg/imsg-tui"

app_src="$root/packaging/mac/Imsg Setup.app"
app_dst="$pkg/Imsg Setup.app"
rm -rf "$app_dst"
cp -R "$app_src" "$app_dst"
mkdir -p "$app_dst/Contents/MacOS"
cp "$pkg/imsg" "$app_dst/Contents/MacOS/imsg"
cp "$pkg/imsg-tui" "$app_dst/Contents/MacOS/imsg-tui"
chmod 755 "$app_dst/Contents/MacOS/"*

rm -rf "$root/dist/imsg-macos-aarch64"
cp -R "$pkg" "$root/dist/imsg-macos-aarch64"
out="$root/dist/imsg-macos-aarch64.tar.gz"
tar -C "$stage" -czf "$out" imsg-macos-aarch64

if command -v shasum >/dev/null; then
  (cd "$root/dist" && shasum -a 256 imsg-macos-aarch64.tar.gz > imsg-macos-aarch64.tar.gz.sha256)
else
  (cd "$root/dist" && sha256sum imsg-macos-aarch64.tar.gz > imsg-macos-aarch64.tar.gz.sha256)
fi

echo "$out"
