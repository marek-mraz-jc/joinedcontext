#!/usr/bin/env python3
"""Soak one broker for hours at a fixed low rate and check it stays right (T-3551).

    python3 dev/soak.py --url http://127.0.0.1:9090 --minutes 120 --pid 1234 --log broker.log
    python3 dev/soak.py --selftest

Each worker owns its entities, so the expected state is known exactly. One
cycle per worker: create an Entity, PATCH its speed, read it back as last
written, query the worker's live Entities with count=true, batch create /
update / delete three more, create and delete a throwaway Subscription,
delete the Entity. A Subscription per worker (watchedAttributes speed,
notificationTrigger attributeUpdated) points at the receiver here, so every
successful PATCH is one notification due.

The run fails on any of:
  - a request answering a status other than the one the operation defines;
  - a read-back or a count that differs from what the worker wrote;
  - fewer notifications received than due (DRAIN_S after the last write);
  - RssAnon of --pid growing past max(25 %, 16 MiB) over its value at the
    end of the warm-up (--warmup, default a tenth of the run);
  - an ERROR or a panic line in --log.
The verdict and numbers go to stdout as Markdown (and to --summary).
"""
import argparse
import json
import os
import random
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MIB = 1024 * 1024
DRAIN_S = 30


# --- judges: pure, so --selftest can plant a failure in each -------------

def judge_rss(series, warm_at):
    """series: [(t_seconds, rss_bytes[, allocated, resident])]. The growth
    after warm-up is the largest sample from `warm_at` on against the sample
    at `warm_at`. The verdict names jemalloc's allocated and resident growth
    over the same window when the series has them, so a red run says where
    the memory is: live Rust allocations (allocated), held by jemalloc
    (resident), or outside jemalloc (neither, T-3614)."""
    after = [row for row in series if row[0] >= warm_at]
    if len(after) < 2:
        return False, "fewer than two RSS samples after warm-up"
    rss = [row[1] for row in after]
    base, peak = rss[0], max(rss)
    allowed = max(base * 0.25, 16 * MIB)
    ok = peak - base <= allowed
    msg = (f"RssAnon {base / MIB:.1f} MiB at warm-up, peak {peak / MIB:.1f} MiB, "
           f"growth {(peak - base) / MIB:.1f} MiB of {allowed / MIB:.1f} allowed")
    for col, name in ((2, "allocated"), (3, "resident")):
        seen = [row[col] for row in after if len(row) > col and row[col] is not None]
        if len(seen) >= 2:
            msg += f"; jemalloc {name} growth {(max(seen) - seen[0]) / MIB:+.1f} MiB"
    return ok, msg


def judge_count(due, received):
    """due / received: {subscription id: notifications}."""
    short = {s: (n, received.get(s, 0)) for s, n in due.items() if received.get(s, 0) < n}
    total_due, total_got = sum(due.values()), sum(received.get(s, 0) for s in due)
    msg = f"{total_got} of {total_due} notifications received"
    if short:
        worst = sorted(short.items())[:3]
        msg += "; short: " + ", ".join(f"{s} {got}/{n}" for s, (n, got) in worst)
    return not short, msg


def judge_readback(written, read):
    """The speed a worker PATCHed against the one GET returned."""
    got = read.get("speed", {})
    got = got.get("value") if isinstance(got, dict) else None
    return got == written, f"wrote speed {written}, read {got!r}"


def judge_log(lines):
    # the broker colours its levels: `\x1b[31mERROR` has no word boundary
    plain = (re.sub(r"\x1b\[[0-9;]*m", "", l) for l in lines)
    bad = [l for l in plain if re.search(r"\bERROR\b|panicked", l)]
    return not bad, f"{len(bad)} ERROR/panic lines" + (f", first: {bad[0].strip()[:200]}" if bad else "")


# --- the receiver ----------------------------------------------------------

class Receiver:
    def __init__(self):
        self.lock = threading.Lock()
        self.by_sub = {}
        outer = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                try:
                    n = json.loads(body)
                    sub, count = n.get("subscriptionId"), len(n.get("data") or [])
                except (ValueError, AttributeError):
                    sub, count = None, 0
                if sub:
                    with outer.lock:
                        outer.by_sub[sub] = outer.by_sub.get(sub, 0) + count
                self.send_response(204)
                self.send_header("Content-Length", "0")
                self.end_headers()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def counts(self):
        with self.lock:
            return dict(self.by_sub)


# --- the load --------------------------------------------------------------

class Broker:
    def __init__(self, url):
        self.base = url.rstrip("/") + "/ngsi-ld/v1"

    def call(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                raw = r.read()
                return r.status, (json.loads(raw) if raw else None), r.headers
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode(errors="replace")[:300], e.headers


class Worker(threading.Thread):
    def __init__(self, k, broker, receiver_url, rate, stop, failures):
        super().__init__(daemon=True)
        self.k, self.b, self.rate, self.stop, self.failures = k, broker, rate, stop, failures
        self.receiver_url = receiver_url
        self.sub = f"urn:ngsi-ld:Subscription:soak:{k}"
        self.due = 0
        self.cycles = 0
        self.requests = 0

    def expect(self, what, status, want):
        self.requests += 1
        if status not in want:
            self.failures.append(f"worker {self.k}: {what}: {status}")
            return False
        return True

    def run(self):
        b, k = self.b, self.k
        st, _, _ = b.call("POST", "/subscriptions", {
            "id": self.sub, "type": "Subscription",
            "entities": [{"type": "SoakCar", "idPattern": f"^urn:ngsi-ld:SoakCar:{k}-"}],
            "watchedAttributes": ["speed"], "notificationTrigger": ["attributeUpdated"],
            "notification": {"endpoint": {"uri": self.receiver_url}}})
        if not self.expect("create subscription", st, (201,)):
            return
        n = 0
        while not self.stop.is_set():
            started = time.monotonic()
            n += 1
            self.cycle(n)
            self.cycles += 1
            # a fixed rate of cycles, whatever the broker's latency
            self.stop.wait(max(0.0, 1.0 / self.rate - (time.monotonic() - started)))

    def cycle(self, n):
        b, k = self.b, self.k
        eid = f"urn:ngsi-ld:SoakCar:{k}-{n}"
        st, _, _ = b.call("POST", "/entities", {
            "id": eid, "type": "SoakCar",
            "owner": {"type": "Property", "value": f"w{k}"},
            "speed": {"type": "Property", "value": 0}})
        if not self.expect("create", st, (201,)):
            return
        speed = random.randint(1, 10_000)
        st, _, _ = b.call("PATCH", f"/entities/{eid}/attrs",
                          {"speed": {"type": "Property", "value": speed}})
        if self.expect("patch", st, (204,)):
            self.due += 1
        st, doc, _ = b.call("GET", f"/entities/{eid}")
        if self.expect("retrieve", st, (200,)):
            ok, why = judge_readback(speed, doc if isinstance(doc, dict) else {})
            if not ok:
                self.failures.append(f"worker {k}: read-back: {why}")
        st, _, headers = b.call(
            "GET", f"/entities?type=SoakCar&q=owner==%22w{k}%22&count=true&limit=1")
        if self.expect("query", st, (200,)):
            count = headers.get("NGSILD-Results-Count")
            if count != "1":
                self.failures.append(f"worker {k}: count: expected 1 live entity, got {count}")
        batch = [f"urn:ngsi-ld:SoakBatch:{k}-{n}-{i}" for i in range(3)]
        docs = [{"id": i, "type": "SoakBatch", "level": {"type": "Property", "value": 1}}
                for i in batch]
        st, _, _ = b.call("POST", "/entityOperations/create", docs)
        self.expect("batch create", st, (201,))
        for d in docs:
            d["level"]["value"] = 2
        st, _, _ = b.call("POST", "/entityOperations/update", docs)
        self.expect("batch update", st, (204,))
        st, _, _ = b.call("POST", "/entityOperations/delete", batch)
        self.expect("batch delete", st, (204,))
        tmp = f"urn:ngsi-ld:Subscription:soak:{k}:tmp"
        st, _, _ = b.call("POST", "/subscriptions", {
            "id": tmp, "type": "Subscription", "entities": [{"type": "SoakNever"}],
            "notification": {"endpoint": {"uri": self.receiver_url}}})
        if self.expect("create throwaway subscription", st, (201,)):
            st, _, _ = b.call("DELETE", f"/subscriptions/{tmp}")
            self.expect("delete throwaway subscription", st, (204,))
        st, _, _ = b.call("DELETE", f"/entities/{eid}")
        self.expect("delete", st, (204,))


def rss_anon(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("RssAnon:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        return None
    return None


def heap(url):
    """jemalloc's allocated and resident bytes from /q/health (T-3614): what
    the program holds live against what the allocator keeps mapped, so a
    step in RssAnon reads as a live allocation or as allocator retention."""
    try:
        with urllib.request.urlopen(url.rstrip("/") + "/q/health", timeout=10) as r:
            mem = json.loads(r.read()).get("memory") or {}
    except (OSError, ValueError):
        return None, None
    return mem.get("allocatedBytes"), mem.get("residentBytes")


def soak(args):
    receiver = Receiver()
    broker = Broker(args.url)
    stop = threading.Event()
    failures = []
    url = f"http://127.0.0.1:{receiver.port}/notify"
    workers = [Worker(k, broker, url, args.rate, stop, failures) for k in range(args.workers)]
    for w in workers:
        w.start()
    start = time.monotonic()
    end = start + args.minutes * 60
    warm_at = (args.warmup if args.warmup is not None else args.minutes / 10) * 60
    series = []
    while time.monotonic() < end and len(failures) < 100:
        if args.pid:
            r = rss_anon(args.pid)
            if r is None:
                failures.append(f"broker pid {args.pid} is gone")
                break
            series.append((time.monotonic() - start, r) + heap(args.url))
        time.sleep(min(10.0, max(0.0, end - time.monotonic())))
    stop.set()
    for w in workers:
        w.join(timeout=60)
    elapsed = time.monotonic() - start
    due = {w.sub: w.due for w in workers}
    deadline = time.monotonic() + DRAIN_S
    while time.monotonic() < deadline and not judge_count(due, receiver.counts())[0]:
        time.sleep(0.5)

    checks = [("requests", not failures,
               f"{sum(w.requests for w in workers)} requests, {sum(w.cycles for w in workers)} cycles, "
               f"{len(failures)} failures" + (f"; first: {failures[0]}" if failures else ""))]
    checks.append(("notifications",) + judge_count(due, receiver.counts()))
    if args.pid:
        checks.append(("memory",) + judge_rss(series, warm_at))
    if args.log:
        with open(args.log, errors="replace") as f:
            checks.append(("log",) + judge_log(f.readlines()))
    ok = all(c[1] for c in checks)
    lines = [f"### soak {'PASS' if ok else 'FAIL'}: {elapsed / 60:.1f} min, "
             f"{args.workers} workers at {args.rate} cycles/s each", "",
             "| check | verdict | numbers |", "|---|---|---|"]
    lines += [f"| {name} | {'PASS' if good else 'FAIL'} | {why} |" for name, good, why in checks]
    text = "\n".join(lines) + "\n"
    print(text)
    if args.summary:
        with open(args.summary, "a") as f:
            f.write(text)
    if args.series and series:
        with open(args.series, "w") as f:
            f.write("seconds,rss_anon_bytes,allocated_bytes,resident_bytes\n")
            f.writelines(f"{t:.0f},{r},{a if a is not None else ''},{m if m is not None else ''}\n"
                         for t, r, a, m in series)
    return 0 if ok else 1


def selftest():
    flat = [(t, 100 * MIB) for t in range(0, 600, 10)]
    assert judge_rss(flat, 60)[0]
    climb = [(t, 100 * MIB + t * 100_000) for t in range(0, 7200, 10)]
    assert not judge_rss(climb, 600)[0], judge_rss(climb, 600)
    # warm-up growth is not held against the run
    warm = [(t, (50 if t < 60 else 100) * MIB) for t in range(0, 600, 10)]
    assert judge_rss(warm, 60)[0]
    assert not judge_rss([(0, MIB)], 0)[0]
    # heap columns ride along; the verdict reads RssAnon alone
    assert judge_rss([(0, MIB, 1, 2), (10, MIB, None, None)], 0)[0]
    # a red verdict says whether the step is in jemalloc's heap or outside it
    step = [(0, 30 * MIB, 4 * MIB, 13 * MIB), (10, 48 * MIB, 4 * MIB, 13 * MIB)]
    ok, msg = judge_rss(step, 0)
    assert not ok and "allocated growth +0.0 MiB" in msg and "resident growth +0.0 MiB" in msg, msg
    assert judge_count({"s": 10}, {"s": 10})[0]
    assert not judge_count({"s": 10}, {"s": 9})[0]
    assert not judge_count({"s": 1, "t": 1}, {"s": 2})[0]
    assert judge_readback(7, {"speed": {"type": "Property", "value": 7}})[0]
    assert not judge_readback(7, {"speed": {"type": "Property", "value": 6}})[0]
    assert not judge_readback(7, {})[0]
    assert judge_log(["INFO started\n"])[0]
    assert not judge_log(["2026 ERROR store failed\n"])[0]
    assert not judge_log(["thread 'tokio' panicked at x\n"])[0]
    assert not judge_log(["\x1b[2m2026\x1b[0m \x1b[31mERROR\x1b[0m store failed\n"])[0]
    assert judge_log(["\x1b[32m INFO\x1b[0m ERRORS_TOTAL=0\n"])[0]
    print("soak selftest: ok")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--url", default="http://127.0.0.1:9090")
    p.add_argument("--minutes", type=float, default=120)
    p.add_argument("--warmup", type=float, help="minutes (default: a tenth of the run)")
    p.add_argument("--workers", type=int, default=4)
    p.add_argument("--rate", type=float, default=2.0, help="cycles per second per worker")
    p.add_argument("--pid", type=int, help="broker pid, for RssAnon")
    p.add_argument("--log", help="broker log to scan")
    p.add_argument("--summary", default=os.environ.get("GITHUB_STEP_SUMMARY"))
    p.add_argument("--series", help="write the RSS samples as CSV")
    p.add_argument("--selftest", action="store_true")
    args = p.parse_args()
    sys.exit(selftest() if args.selftest else soak(args))


if __name__ == "__main__":
    main()
