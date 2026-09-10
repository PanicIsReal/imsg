#!/usr/bin/env python3
"""Measure native watch latency using synthetic messages in a temporary database."""

import argparse
import json
import os
import selectors
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

SCHEMA = """
CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT, service TEXT, country TEXT, uncanonicalized_id TEXT);
CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, chat_identifier TEXT, display_name TEXT, service_name TEXT, style INTEGER);
CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT, text TEXT, handle_id INTEGER, date INTEGER, date_read INTEGER, date_delivered INTEGER, is_from_me INTEGER, is_read INTEGER, service TEXT, attributedBody BLOB, cache_has_attachments INTEGER, associated_message_guid TEXT, associated_message_type INTEGER, item_type INTEGER, group_action_type INTEGER);
CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
CREATE TABLE attachment (ROWID INTEGER PRIMARY KEY, guid TEXT, filename TEXT, mime_type TEXT, transfer_name TEXT, total_bytes INTEGER);
CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
INSERT INTO handle (ROWID,id,service) VALUES(1,'test@example.invalid','iMessage');
INSERT INTO chat VALUES(1,'iMessage;-;test@example.invalid','test@example.invalid','Fixture','iMessage',45);
INSERT INTO chat_handle_join VALUES(1,1);
"""


def run(imsg, debounce_ms, samples):
    with tempfile.TemporaryDirectory(prefix="imsg-watch-latency-") as directory:
        database = Path(directory) / "chat.db"
        connection = sqlite3.connect(database)
        connection.execute("PRAGMA journal_mode=WAL")
        connection.executescript(SCHEMA)
        process = subprocess.Popen(
            [imsg, "rpc", "--db", str(database)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            bufsize=0,
        )
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        buffer = b""
        frames = []

        def receive():
            nonlocal buffer
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if frames:
                    return frames.pop(0)
                for key, _ in selector.select(0.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        raise RuntimeError("native RPC closed stdout")
                    buffer += chunk
                    while b"\n" in buffer:
                        line, buffer = buffer.split(b"\n", 1)
                        if line:
                            frames.append(json.loads(line))
            raise TimeoutError("native RPC did not emit the expected frame within 10s")

        try:
            request = {"jsonrpc": "2.0", "id": "watch", "method": "watch.subscribe",
                       "params": {"debounce_ms": debounce_ms, "since_rowid": 0}}
            process.stdin.write((json.dumps(request) + "\n").encode())
            process.stdin.flush()
            response = receive()
            if response.get("id") != "watch" or "error" in response:
                raise RuntimeError(f"watch subscription failed: {response}")
            for index in range(samples):
                guid = f"latency-fixture-{index}"
                connection.execute(
                    "INSERT INTO message (guid,text,handle_id,date,is_from_me,is_read,service,cache_has_attachments,item_type,associated_message_type,group_action_type) VALUES(?,?,1,?,0,0,'iMessage',0,0,0,0)",
                    (guid, "Synthetic latency fixture", int((time.time() - 978307200) * 1e9)),
                )
                rowid = connection.execute("SELECT last_insert_rowid()").fetchone()[0]
                connection.execute("INSERT INTO chat_message_join VALUES(1,?)", (rowid,))
                started = time.monotonic()
                connection.commit()
                while True:
                    event = receive()
                    message = event.get("params", {}).get("message", {})
                    if message.get("guid") == guid:
                        break
                    if "error" in event:
                        raise RuntimeError(f"watch failed: {event}")
                print(json.dumps({"metric": f"native_db_to_watch_{debounce_ms}ms",
                                  "elapsed_ms": (time.monotonic() - started) * 1000}), flush=True)
        finally:
            selector.close()
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            process.stdin.close()
            process.stdout.close()
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imsg", required=True, help="path to the native steipete imsg binary")
    parser.add_argument("--debounce-ms", type=int, default=100)
    parser.add_argument("--samples", type=int, default=20)
    args = parser.parse_args()
    if args.samples < 1 or args.debounce_ms < 1:
        parser.error("samples and debounce-ms must be positive")
    run(args.imsg, args.debounce_ms, args.samples)


if __name__ == "__main__":
    main()
