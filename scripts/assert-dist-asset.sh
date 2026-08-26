#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
role="${1:-}"

case "$role" in
  linux)
    archive="$root/dist/imsg-linux-x86_64.tar.gz"
    [[ -f "$archive" ]]
    [[ -s "$archive.sha256" ]]
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    tar -xzf "$archive" -C "$tmp"
    file "$tmp/imsg" | grep -E -q 'ELF.*x86-64|ELF.*x86_64'
    ;;
  mac)
    archive="$root/dist/imsg-macos-aarch64.tar.gz"
    [[ -f "$archive" ]]
    [[ -s "$archive.sha256" ]]
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    tar -xzf "$archive" -C "$tmp"
    file "$tmp/imsg-macos-aarch64/imsg" | grep -q 'Mach-O 64-bit executable arm64'
    [[ -x "$tmp/imsg-macos-aarch64/imsg-tui" ]]
    [[ -x "$tmp/imsg-macos-aarch64/Imsg Setup.app/Contents/MacOS/ImsgSetup" ]]
    ;;
  *)
    echo "usage: $0 linux|mac" >&2
    exit 1
    ;;
esac

echo "assert-dist-asset $role ok"
