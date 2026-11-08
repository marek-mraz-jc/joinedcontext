#!/usr/bin/env python3
"""Scale-out on one JetStream consumer: 1, 2 and 3 instances of ekiben and of Bento 1.22 share
a queue group (push consumer with a deliver group, Bento's scale-out pattern) on one stream.

Per tool: three throughput runs (1, 2, 3 instances, nothing killed) and two kill runs (2 and 3
instances, instance 0 SIGKILLed once 40 % of the messages are written). Each run uses a fresh
queue group over the same N stored messages, so every run reads all N. A run ends when every
message id was written once or nothing new arrived for ack_wait + 5 s.

Reports per run: wall time from the first written line to the last, msg/s, lost (ids never
written) and duplicates (lines minus distinct ids). Exits 1 when ekiben loses a message.

Usage: scripts/nats-parity/scale.py --bento .bento/bento --ekiben target/release/ekiben [-n 100000]
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import time

NATS = "nats:2.12-alpine"
BOX = "natsio/nats-box:0.18.1"
ACK_WAIT = 5

CONF = """
input:
  nats_jetstream:
    urls: [ {url} ]
    subject: scale.>
    queue: {queue}
    deliver: all
    ack_wait: {ack_wait}s
pipeline:
  processors:
    - mapping: |
        root = this
        root.square = this.n * this.n
        root.label = "n-" + this.n.string()
output:
  file:
    path: {path}
    codec: lines
"""


def sh(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def nats(url, *args):
    out = sh(["docker", "run", "--rm", "--network", "host", BOX, "nats", "--server", url, *args])
    if out.returncode:
        raise RuntimeError(f"nats {args}: {out.stderr}")
    return out.stdout


def count_lines(paths):
    total = 0
    for p in paths:
        try:
            with open(p, "rb") as f:
                total += sum(chunk.count(b"\n") for chunk in iter(lambda: f.read(1 << 20), b""))
        except FileNotFoundError:
            pass
    return total


def ids_of(paths):
    ids, lines = set(), 0
    for p in paths:
        if not os.path.exists(p):
            continue
        with open(p) as f:
            for line in f:
                try:
                    ids.add(json.loads(line)["n"])
                    lines += 1
                except (json.JSONDecodeError, KeyError):
                    pass  # a line cut by SIGKILL: its message was not acked and comes again
    return ids, lines


def run(tool, cmd, url, n, k, kill, run_id, work):
    paths, procs = [], []
    queue = f"q{run_id}"
    for i in range(k):
        path = os.path.join(work, f"{run_id}-{i}.jsonl")
        conf = os.path.join(work, f"{run_id}-{i}.yaml")
        with open(conf, "w") as f:
            f.write(CONF.format(url=url, queue=queue, ack_wait=ACK_WAIT, path=path))
        args = [cmd, "-c", conf] if tool == "bento" else [cmd, "run", conf]
        log = open(os.path.join(work, f"{run_id}-{i}.log"), "w")
        procs.append(subprocess.Popen(args, stdout=log, stderr=log))
        paths.append(path)
    start = time.time()
    first = last = None
    seen = 0
    last_change = time.time()
    killed = False
    while True:
        time.sleep(0.05)
        now_lines = count_lines(paths)
        if now_lines and first is None:
            first = time.time()
        if now_lines != seen:
            seen, last_change, last = now_lines, time.time(), time.time()
        if kill and not killed and seen >= 0.4 * n:
            procs[0].send_signal(signal.SIGKILL)
            killed = True
        if seen >= n and not kill:
            break
        if kill and killed and seen >= n:
            ids, _ = ids_of(paths)
            if len(ids) >= n:
                break
        if time.time() - last_change > ACK_WAIT + 5 or time.time() - start > 300:
            break
    for p in procs:
        if p.poll() is None:
            p.terminate()
    for p in procs:
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
    ids, lines = ids_of(paths)
    wall = (last or time.time()) - (first or start)
    per_instance = [count_lines([p]) for p in paths]
    return {"tool": tool, "instances": k, "killed": kill, "wall_s": round(wall, 2),
            "msg_per_s": round(len(ids) / wall) if wall > 0 else 0,
            "lost": n - len(ids), "duplicates": lines - len(ids), "per_instance": per_instance}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bento", default=".bento/bento")
    ap.add_argument("--ekiben", required=True)
    ap.add_argument("-n", type=int, default=100000)
    a = ap.parse_args()
    if sh(["docker", "info"]).returncode:
        print("SKIP nats scale: docker is not available")
        return 0
    out = sh(["docker", "run", "-d", "--rm", "-p", "127.0.0.1::4222", NATS, "-js"])
    cid = out.stdout.strip()
    try:
        port = sh(["docker", "port", cid, "4222"]).stdout.splitlines()[0].rsplit(":", 1)[1]
        url = f"nats://127.0.0.1:{port}"
        time.sleep(1)
        nats(url, "stream", "add", "SCALE", "--subjects", "scale.>", "--defaults")
        nats(url, "pub", "scale.x", '{"n":{{Count}},"pad":"0123456789abcdef0123456789abcdef"}',
             "--count", str(a.n), "--jetstream")
        stored = json.loads(nats(url, "stream", "info", "SCALE", "--json"))["state"]["messages"]
        print(f"load {os.getloadavg()[0]:.2f} {os.getloadavg()[1]:.2f} {os.getloadavg()[2]:.2f}, "
              f"{os.cpu_count()} cores, {stored} messages stored")
        assert stored == a.n, f"stream holds {stored}, wanted {a.n}"
        work = tempfile.mkdtemp(prefix="ekiben-scale-")
        rows = []
        run_id = 0
        for tool, cmd in (("bento", os.path.abspath(a.bento)), ("ekiben", os.path.abspath(a.ekiben))):
            for k, kill in ((1, False), (2, False), (3, False), (2, True), (3, True)):
                run_id += 1
                r = run(tool, cmd, url, a.n, k, kill, run_id, work)
                rows.append(r)
                print(json.dumps(r), flush=True)
        print("\n| tool | instances | killed | wall s | msg/s | lost | duplicates | per instance |")
        print("|---|---|---|---|---|---|---|---|")
        for r in rows:
            print(f"| {r['tool']} | {r['instances']} | {'yes' if r['killed'] else 'no'} | {r['wall_s']} | "
                  f"{r['msg_per_s']:,} | {r['lost']} | {r['duplicates']} | {r['per_instance']} |")
        bad = [r for r in rows if r["tool"] == "ekiben" and r["lost"]]
        return 1 if bad else 0
    finally:
        sh(["docker", "rm", "-f", cid])


if __name__ == "__main__":
    sys.exit(main())
