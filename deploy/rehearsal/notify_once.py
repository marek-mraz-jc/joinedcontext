#!/usr/bin/env python3
"""notify_once.py --base <url> [--base <url>]... [--writes 20]: each write notifies once.

The split layout's broker check (T-7014): one NGSI-LD subscription made on the first base, then
--writes updates of one entity spread round robin over every base (each a broker replica's own
listener, so every replica takes writes), and a receiver on loopback counts the notifications per
written value. Passes when each value arrived exactly once: a replica that misses the others'
writes loses notifications, two replicas firing one write double them. Run on a jc on allow-all
(up.sh REHEARSAL_IMPORT_POLICY=allow-all, loopback only) with ANTARES_EGRESS_ALLOW_PRIVATE=true,
so the broker may notify a loopback receiver. The probe tenant's subscription and entity are
deleted at the end. Loopback bases only; prints one JSON line."""

import argparse
import http.server
import json
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid

TENANT = "rehearsal-notify-once"


def tally(notifications, writes):
    """Per written value 0..writes-1 how often it arrived; values outside are the initial state."""
    seen = {v: 0 for v in range(writes)}
    for note in notifications:
        for entity in note.get("data", []):
            value = (entity.get("seq") or {}).get("value")
            if isinstance(value, int) and value in seen:
                seen[value] += 1
    return {
        "missing": sorted(v for v, n in seen.items() if n == 0),
        "doubled": sorted(v for v, n in seen.items() if n > 1),
    }


def loopback(url):
    host = urllib.parse.urlsplit(url).hostname
    if host not in ("127.0.0.1", "localhost", "::1"):
        raise SystemExit(f"notify_once: {url} is not a loopback base")
    return url.rstrip("/")


def call(method, url, body=None):
    data = None if body is None else json.dumps(body).encode()
    headers = {"NGSILD-Tenant": TENANT, "Content-Type": "application/json"}
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status
    except urllib.error.HTTPError as e:
        return e.code


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", action="append", required=True, help="a broker replica's jc base")
    ap.add_argument("--writes", type=int, default=20)
    ap.add_argument("--wait", type=float, default=15, help="seconds to wait for the last notification")
    a = ap.parse_args(argv)
    bases = [loopback(b) for b in a.base]

    received = []
    lock = threading.Lock()

    class Receiver(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            size = min(int(self.headers.get("Content-Length") or 0), 1 << 20)
            try:
                note = json.loads(self.rfile.read(size) or b"{}")
            except ValueError:
                note = {}
            with lock:
                received.append(note)
            self.send_response(204)
            self.end_headers()

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    receiver = f"http://127.0.0.1:{server.server_address[1]}/notify"

    run = uuid.uuid4().hex[:12]
    entity = f"urn:ngsi-ld:NotifyOnce:{run}"
    sub = f"urn:ngsi-ld:Subscription:notify-once-{run}"
    broker = lambda b: f"{b}/broker/ngsi-ld/v1"
    out = {"bases": len(bases), "writes": a.writes}
    try:
        st = call("POST", f"{broker(bases[0])}/entities",
                  {"id": entity, "type": "NotifyOnce", "seq": {"type": "Property", "value": -1}})
        if st != 201:
            raise SystemExit(f"notify_once: entity create answered {st}")
        st = call("POST", f"{broker(bases[0])}/subscriptions", {
            "id": sub, "type": "Subscription", "entities": [{"type": "NotifyOnce", "id": entity}],
            "watchedAttributes": ["seq"],
            "notification": {"endpoint": {"uri": receiver, "accept": "application/json"}}})
        if st != 201:
            raise SystemExit(f"notify_once: subscription create answered {st}")
        time.sleep(2)  # every replica learns the subscription (bus or store)
        for i in range(a.writes):
            st = call("PATCH", f"{broker(bases[i % len(bases)])}/entities/{urllib.parse.quote(entity, safe='')}/attrs",
                      {"seq": {"type": "Property", "value": i}})
            if st != 204:
                raise SystemExit(f"notify_once: write {i} on base {i % len(bases)} answered {st}")
        deadline = time.monotonic() + a.wait
        while time.monotonic() < deadline:
            with lock:
                if not tally(received, a.writes)["missing"]:
                    break
            time.sleep(0.5)
        time.sleep(3)  # a double comes after the first
        with lock:
            out.update(tally(received, a.writes), notifications=len(received))
    finally:
        call("DELETE", f"{broker(bases[0])}/subscriptions/{urllib.parse.quote(sub, safe='')}")
        call("DELETE", f"{broker(bases[0])}/entities/{urllib.parse.quote(entity, safe='')}")
        server.shutdown()
    print(json.dumps(out))
    return 1 if out["missing"] or out["doubled"] else 0


if __name__ == "__main__":
    sys.exit(main())
