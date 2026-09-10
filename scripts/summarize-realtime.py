#!/usr/bin/env python3
"""Summarize realtime latency logs from stdin or files."""

import argparse
import json
import math
import re
import sys
from collections import defaultdict


def sample(line):
    line = re.sub(r"\x1b\[[0-9;]*m", "", line)
    start = line.find("{")
    if start >= 0:
        try:
            record, _ = json.JSONDecoder().raw_decode(line[start:])
            fields = record.get("fields", record)
            metric = fields.get("metric")
            elapsed = fields.get("elapsed_ms")
            if isinstance(metric, str) and elapsed is not None:
                return metric, float(elapsed)
        except (ValueError, TypeError, AttributeError):
            pass
    metric = re.search(r'\bmetric=(?:"([\w.:-]+)"|([\w.:-]+))', line)
    elapsed = re.search(r"\belapsed_ms=([0-9]+(?:\.[0-9]+)?)\b", line)
    if metric and elapsed:
        return metric.group(1) or metric.group(2), float(elapsed.group(1))
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="*", help="log files; defaults to stdin")
    args = parser.parse_args()
    samples = defaultdict(list)
    for path in args.files or ["-"]:
        stream = sys.stdin if path == "-" else open(path, encoding="utf-8", errors="replace")
        try:
            for line in stream:
                value = sample(line)
                if value and math.isfinite(value[1]) and value[1] >= 0:
                    samples[value[0]].append(value[1])
        finally:
            if stream is not sys.stdin:
                stream.close()
    if not samples:
        parser.exit(1, "No realtime latency samples found.\n")
    print("metric\tcount\tp50_ms\tp95_ms\tmax_ms")
    for metric, values in sorted(samples.items()):
        values.sort()
        p50 = values[math.ceil(len(values) * 0.50) - 1]
        p95 = values[math.ceil(len(values) * 0.95) - 1]
        print(f"{metric}\t{len(values)}\t{p50:.3f}\t{p95:.3f}\t{values[-1]:.3f}")


if __name__ == "__main__":
    main()
