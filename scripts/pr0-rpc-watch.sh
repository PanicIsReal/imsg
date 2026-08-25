#!/usr/bin/env bash
# Spike imsg rpc JSON-RPC over stdio (watch.subscribe).
set -euo pipefail

DURATION="${1:-5}"

python3 <<'PY' "$DURATION"
import json, subprocess, sys, time

duration = int(sys.argv[1])
proc = subprocess.Popen(
    ["imsg", "rpc"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)

def send(obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()

send({"jsonrpc": "2.0", "id": "1", "method": "initialize", "params": {}})
send({"jsonrpc": "2.0", "id": "2", "method": "status", "params": {}})
send({"jsonrpc": "2.0", "id": "3", "method": "watch.subscribe", "params": {"debounce_ms": 500}})

deadline = time.time() + duration
while time.time() < deadline:
    line = proc.stdout.readline()
    if not line:
        break
    print(line.rstrip())

proc.terminate()
PY
