# Realtime messaging

The plugin displays queued sends immediately and keeps the composer available. Each send attempt has a local `client_id`. The sync daemon echoes that ID in its response and in the confirmed message event when the native RPC supplies a message ID.

Confirmed messages replace their pending bubbles by identity. An acknowledgement without a message ID remains unconfirmed. The plugin never matches messages by text and never retries a send automatically. The outbox lives in the plugin process and does not survive a shell restart.

A database generation change clears the selected conversation and disables retries for unresolved sends that refer to old conversation IDs.

History loads merge into a per-chat message cache. Each load records the selected-chat generation and the chat's live-message revision. A response cannot replace another conversation or overwrite a live update that arrived during the request. Failed and unconfirmed sends remain visible. New messages follow the bottom of the thread only when the user is already near the bottom or sends a message.

The Mac watch debounce is 100 ms. Bridge requests run concurrently with event forwarding. Sync sends a heartbeat every 10 seconds and reconnects after 30 seconds without inbound traffic. Reconnect attempts begin below 500 ms and back off with jitter to a maximum of 5 seconds. A native `watch.overflow`, an RPC child restart, or an internal event queue overflow emits `watch.gap`. Sync then runs `messages.after` catch-up and sends `sync.resync`, which reloads the open conversation.

## Measured receive latency

The native watcher was tested on macOS with steipete `imsg` 0.14.1 and synthetic rows in a temporary SQLite database. No messages were sent, and no personal Messages database was opened.

| Watch debounce | Samples | p50 | p95 |
| --- | ---: | ---: | ---: |
| 500 ms | 20 | 533.357 ms | 539.830 ms |
| 100 ms | 20 | 107.063 ms | 120.481 ms |

The measurement starts immediately before committing a synthetic message and ends when its watch event reaches the probe. It includes the SQLite commit and native watcher, but excludes WSS, sync, and QML rendering. These samples do not establish CPU cost, burst behavior, or end-to-end delivery latency. Raw samples are in [evidence/realtime](evidence/realtime/).

The [native RPC documentation](https://raw.githubusercontent.com/steipete/imsg/main/docs/rpc.md) explains that the higher default debounce allows follow-up database writes to settle. A real-device check should cover outbound attribution as well as inbound latency.

## Repeat the receive measurement

Run these commands on the Mac with the native steipete binary. The probe creates and deletes its own database.

```sh
python3 scripts/probe-watch-latency.py --imsg /opt/homebrew/bin/imsg --debounce-ms 500 --samples 20 > /tmp/watch-500.jsonl
python3 scripts/probe-watch-latency.py --imsg /opt/homebrew/bin/imsg --debounce-ms 100 --samples 20 > /tmp/watch-100.jsonl
python3 scripts/summarize-realtime.py /tmp/watch-500.jsonl /tmp/watch-100.jsonl
```

## Inspect application latency

Set `RUST_LOG=imsg_bridge=debug,imsg_sync=debug` when starting the unified CLI. Its logs go to stderr. The metrics use local monotonic clocks and contain no message text.

| Metric | Measured interval |
| --- | --- |
| `rpc_roundtrip` | Bridge native RPC call start through response or timeout |
| `send_roundtrip` | Sync uplink send call through successful response |
| `cache_commit` | Sync message handling through cache and recovery-cursor writes |
| `event_forward` | Insertion into the local uplink event queue only |

Set `IMSG_REALTIME_METRICS=1` in the shell environment before starting Omarchy to enable the plugin's console metrics. `send_to_model_wall` starts in `sendMessage` and ends after inserting the optimistic message. `event_to_model_wall` starts when the stdout parser invokes `ingest` and covers JSON parsing and reducer application. These metrics use the wall clock. They exclude transport time before `ingest` and do not measure a rendered frame.

While the panel is open, `send_to_next_frame_wall` and `event_to_next_frame_wall` extend those intervals to the next window `frameSwapped` callback. This is a frame queued for presentation, not proof of physical display time. These hooks have not been exercised on Omarchy.

Pass captured logs to `python3 scripts/summarize-realtime.py <logfile>` to compute count, p50, p95, and maximum latency for each metric. The tool accepts both text tracing output and JSON records. Do not add stage percentiles together or subtract clocks from different machines to estimate end-to-end latency.

## Verify changes

```sh
cargo test --workspace
node scripts/test-plugin-realtime.mjs
```

On Omarchy, also verify rapid chat switching, two consecutive sends, a failed send, scroll position while receiving, pagination, and recovery after restarting the Mac bridge. A native Omarchy/QML session was unavailable on the development Mac, so the automated reducer tests do not replace this UI check.
