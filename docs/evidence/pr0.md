# PR-0 Prototype Evidence

**Date:** 2026-08-25  
**Machine:** macOS (FDA granted to `imsg` and terminal)

## Commands run

```bash
./scripts/pr0-prototype.sh docs/evidence/pr0-output.ndjson
./scripts/pr0-rpc-watch.sh 3
```

## Results

| Check | Result |
|-------|--------|
| `imsg chats --limit 3` | OK — returned chat list with ids 368, 2, 26 |
| `imsg history --chat-id 368 --limit 5` | OK — 5 messages in chronological order |
| `imsg watch` (5s window) | OK — no errors; quiet period (no new messages during capture) |
| `imsg rpc` + `watch.subscribe` | OK — subscription accepted via JSON-RPC |

## Pagination notes

- History returns chronological order (oldest first within window).
- For infinite scroll up, use `messages.history` with `before` ISO8601 cursor (confirmed in imsg RPC docs).
- ROWID cursors from `messages.after` are for forward catch-up only.

## Latency

Not measured in this capture (no inbound message during watch window). Target for PR-1 live verify: p95 < 500ms Mac DB → WSS emit.

## Conclusion

`imsg` CLI and RPC are viable data sources. Proceed with Rust `imsg-bridge` wrapping `imsg rpc`.
