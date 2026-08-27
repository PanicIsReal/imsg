#!/usr/bin/env python3
import json
import os
import socket
import sys


def socket_path():
    return f"/run/user/{os.getuid()}/imsg-sync.sock"


def request(method, params=None):
    if params is None:
        params = {}
    req = {"type": "req", "id": "1", "method": method, "params": params}
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(socket_path())
    sock.sendall((json.dumps(req, separators=(",", ":")) + "\n").encode())
    buf = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        buf += chunk
        if b"\n" in buf:
            break
    sock.close()
    return buf.split(b"\n", 1)[0].decode()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(
            json.dumps(
                {
                    "type": "res",
                    "id": "1",
                    "ok": False,
                    "error": {
                        "code": "usage",
                        "message": "usage: request.py <method>  (params JSON on stdin)",
                    },
                }
            )
        )
        sys.exit(1)
    method = sys.argv[1]
    raw = sys.stdin.readline() or "{}"
    params = json.loads(raw)
    print(request(method, params))
