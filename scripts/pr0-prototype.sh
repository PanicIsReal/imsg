#!/usr/bin/env bash
# PR-0 prototype: exercise imsg rpc watch + history locally.
# Reviewers rerun: ./scripts/pr0-prototype.sh
set -euo pipefail

OUT="${1:-docs/evidence/pr0-output.ndjson}"
mkdir -p "$(dirname "$OUT")"

echo "=== imsg chats (limit 3) ==="
imsg chats --limit 3 --json

CHAT_ID="${CHAT_ID:-$(imsg chats --limit 1 --json | head -1 | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')}"
echo "=== history chat_id=$CHAT_ID limit 5 ==="
imsg history --chat-id "$CHAT_ID" --limit 5 --json

echo "=== watch 5s (all chats) ==="
timeout 5 imsg watch --json 2>/dev/null | tee "$OUT" || true

echo "Output saved to $OUT"
echo "Grant FDA to imsg and terminal if commands fail."
