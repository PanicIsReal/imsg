#!/usr/bin/env python3
import json
import os
import socket
import sys


def socket_path():
    return os.environ.get("IMSG_SYNC_SOCKET") or os.path.join(
        os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"), "imsg-sync.sock"
    )


def main():
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(20)
    sock.connect(socket_path())
    req = {"type": "req", "id": "1", "method": "events.subscribe", "params": {}}
    sock.sendall((json.dumps(req, separators=(",", ":")) + "\n").encode())
    buf = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            sys.exit(1)
        buf += chunk
        if len(buf) > 16 * 1024 * 1024:
            raise ValueError("event frame too large")
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            if not line:
                continue
            sock.settimeout(None)
            sys.stdout.write(line.decode() + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
