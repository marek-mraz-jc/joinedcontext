#!/usr/bin/env python3
"""memwatch.py --pid-file <jc.pid> --base <http://localhost:port> --out <csv> [--every 5] [--token-file f]

Samples one jc process for a memory measurement (jc-memory-1g): VmRSS, VmHWM, RssAnon and RssFile
from /proc/<pid>/status, and the allocator's allocated and resident bytes plus the Antares answer
pool from the broker's /q/health (jemalloc stats of the whole process: one global allocator).
One CSV row per sample, sizes in KiB. The pid file is reread each sample (up.sh restarts jc); runs
until killed. Loopback bases only."""

import argparse
import csv
import json
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

FIELDS = ("VmRSS", "VmHWM", "RssAnon", "RssFile", "RssShmem", "VmSwap", "Threads")


def proc(pid):
    out = {}
    with open(f"/proc/{pid}/status", encoding="ascii") as f:
        for line in f:
            key, _, val = line.partition(":")
            if key in FIELDS:
                out[key] = int(val.split()[0])
    return out


def health(base, token):
    req = urllib.request.Request(f"{base}/broker/q/health")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            body = json.load(r)
    except (OSError, ValueError):
        return {}
    mem = body.get("memory") or {}
    pool = mem.get("answerPool") or {}
    return {
        "allocated": mem.get("allocatedBytes", 0) // 1024,
        "resident": mem.get("residentBytes", 0) // 1024,
        "poolHeld": pool.get("heldBytes", 0) // 1024,
    }


def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--pid-file", required=True)
    p.add_argument("--base", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--every", type=float, default=5)
    p.add_argument("--token-file")
    p.add_argument("--label", default="")
    a = p.parse_args(argv)
    host = urllib.parse.urlsplit(a.base).hostname
    if host not in ("localhost", "127.0.0.1", "::1"):
        print(f"refusing {a.base}: loopback only", file=sys.stderr)
        return 2
    token = (
        Path(a.token_file).read_text(encoding="ascii").strip() if a.token_file else None
    )
    cols = ("ts", "label", "pid", *FIELDS, "allocated", "resident", "poolHeld")
    with open(a.out, "a", newline="", encoding="ascii") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        if f.tell() == 0:
            w.writeheader()
        while True:
            try:
                pid = int(Path(a.pid_file).read_text(encoding="ascii").strip())
                row = proc(pid)
            except (OSError, ValueError):
                time.sleep(a.every)
                continue
            row["pid"] = pid
            row.update(health(a.base, token))
            row["ts"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            row["label"] = a.label
            w.writerow(row)
            f.flush()
            time.sleep(a.every)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
