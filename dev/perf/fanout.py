#!/usr/bin/env python3
"""Notification fan-out: what one update costs when S subscriptions match it.

    python3 dev/perf/fanout.py http://127.0.0.1:9471 http://127.0.0.1:9800 1 100 10000

Per S, in a tenant of its own: S Subscriptions on Vehicle/speed with the
sink (dev/perf/sink.py) as endpoint, one Vehicle, then UPDATES (10) PATCHes
one after the other. The row is the PATCH p50/p99 as the client saw it, the
notifications due (UPDATES x S) and delivered, and the seconds from the
first PATCH until the sink held the last one (or DRAIN_S, 120, ran out),
which gives notifications per second. The broker needs
ANTARES_EGRESS_ALLOW_PRIVATE=true to deliver to a loopback sink.
"""
import json
import os
import sys
import time
import uuid
import urllib.request

UPDATES = int(os.environ.get("UPDATES", "10"))
DRAIN_S = float(os.environ.get("DRAIN_S", "120"))


def call(method, url, body=None, tenant=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if tenant:
        req.add_header("NGSILD-Tenant", tenant)
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.status, r.read()


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, round(p / 100 * (len(xs) - 1)))]


def row(broker, sink, subs):
    base = f"{broker}/ngsi-ld/v1"
    tenant = f"fanout{subs}x{uuid.uuid4().hex[:8]}"
    car = "urn:ngsi-ld:Vehicle:fanout"
    call("POST", f"{base}/entities", {"id": car, "type": "Vehicle",
                                      "speed": {"type": "Property", "value": 0}}, tenant)
    for k in range(subs):
        call("POST", f"{base}/subscriptions", {
            "id": f"urn:ngsi-ld:Subscription:fanout:{k}", "type": "Subscription",
            "entities": [{"type": "Vehicle"}], "watchedAttributes": ["speed"],
            "notification": {"endpoint": {"uri": f"{sink}/notify"}}}, tenant)
    call("DELETE", f"{sink}/stats")
    lat = []
    start = time.monotonic()
    for i in range(1, UPDATES + 1):
        t = time.monotonic()
        status, _ = call("PATCH", f"{base}/entities/{car}/attrs",
                         {"speed": {"type": "Property", "value": i}}, tenant)
        lat.append((time.monotonic() - t) * 1000)
        if status != 204:
            sys.exit(f"PATCH {i}: {status}")
    due = UPDATES * subs
    delivered = 0
    while time.monotonic() - start < DRAIN_S:
        delivered = json.loads(call("GET", f"{sink}/stats")[1])["entities"]
        if delivered >= due:
            break
        time.sleep(0.05)
    took = time.monotonic() - start
    return (f"| {subs} | {pct(lat, 50):.1f} ms | {pct(lat, 99):.1f} ms | {due} | {delivered} "
            f"| {100 * delivered / due:.1f} | {took:.2f} s | {delivered / took:.0f} |")


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    broker, sink = sys.argv[1].rstrip("/"), sys.argv[2].rstrip("/")
    print("| subscriptions | PATCH p50 | PATCH p99 | due | delivered | delivered % | until the last | notifications/s |")
    print("|---|---|---|---|---|---|---|---|")
    for subs in map(int, sys.argv[3:]):
        print(row(broker, sink, subs), flush=True)


if __name__ == "__main__":
    main()
