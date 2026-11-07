#!/usr/bin/env python3
"""The dev OOM mix (T-4549): exports read page by page while a batch upsert flood runs and
subscriptions with slow receivers fall behind, against a broker under a memory limit.

    python3 dev/perf/oom.py http://127.0.0.1:9090 <broker container> [seconds]

What dev ran on 2026-10-18 when the broker was OOMKilled at 1536Mi three times: one tenant of
ENTITIES helsinki-sized entities behind a view, read the way the gateway reads a download (pages of
100, four at a time, with count=true), a batch upsert of BATCH entities FLOOD times a second into
the same tenant, and SUBS subscriptions on that type whose receiver answers after SLOW_S seconds
(a gateway that went away, a busy receiver), so each one's delivery lane backs up. The receiver
is in this process. Every second the container's memory is read from `docker stats`; the run
fails when the container dies (OOMKilled) and prints the peak, the read p95 and the counts.

Env: ENTITIES (10000), BATCH (100), FLOOD (20), SUBS (20), SLOW_S (2), READERS (4), FLOODERS (4,
the upserts in flight at once), DISCOVERERS (0, clients asking the view for /types and
/attributes with details, the whole-tenant fold that OOMKilled dev at start, T-4562).

The broker's allocator settings matter to the result: run it with the image's
`_RJEM_MALLOC_CONF` (Dockerfile) or the peak reads higher than dev's.
"""

import json
import os
import random
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = sys.argv[1].rstrip("/") if len(sys.argv) > 1 else "http://127.0.0.1:9090"
CONTAINER = sys.argv[2] if len(sys.argv) > 2 else "antares-w1"
SECONDS = int(sys.argv[3]) if len(sys.argv) > 3 else 120
ENTITIES = int(os.environ.get("ENTITIES", "10000"))
BATCH = int(os.environ.get("BATCH", "100"))
FLOOD = float(os.environ.get("FLOOD", "20"))
SUBS = int(os.environ.get("SUBS", "20"))
SLOW_S = float(os.environ.get("SLOW_S", "2"))
READERS = int(os.environ.get("READERS", "4"))
FLOODERS = int(os.environ.get("FLOODERS", "4"))
DISCOVERERS = int(os.environ.get("DISCOVERERS", "0"))
TENANT = os.environ.get("TENANT", "oomrig")
TYPE = "CityWork"
SINK_PORT = 42079


def call(method, path, body=None, timeout=60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method)
    req.add_header("NGSILD-Tenant", TENANT)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as answer:
            return answer.status, answer.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except OSError as e:
        return 0, str(e).encode()


def entity(n, version):
    """A CityWork shaped like helsinki's: a dozen text properties, a polygon, about 3 KB."""
    lon, lat = 24.8 + (n % 400) / 1000, 60.1 + (n // 400) / 1000
    ring = [[lon + dx / 2000, lat + dy / 2000] for dx, dy in ((0, 0), (1, 0), (1, 1), (0, 1), (0, 0))]
    words = " ".join(random.choice(["katu", "työmaa", "kaivu", "liikenne", "ohjaus"]) for _ in range(40))
    props = {f"field{k}": {"type": "Property", "value": f"{words} {n} {version} {k}"} for k in range(8)}
    return {
        "id": f"urn:ngsi-ld:{TYPE}:{n}",
        "type": TYPE,
        "status": {"type": "Property", "value": random.choice(["planned", "ongoing", "done"])},
        "version": {"type": "Property", "value": version},
        "location": {"type": "GeoProperty", "value": {"type": "Polygon", "coordinates": [ring]}},
        **props,
    }


class Slow(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        time.sleep(SLOW_S)
        self.send_response(204)
        self.end_headers()

    def log_message(self, *args):
        pass


def setup():
    view = {
        "id": TENANT, "tenant": TENANT, "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["retrieve_entity", "query_entities", "query_entities_post", "retrieve_entity_types",
                       "retrieve_entity_type_details", "retrieve_attributes", "retrieve_attribute_details"],
    }
    status, body = call("PUT", f"/q/views/{TENANT}", view)
    assert status in (201, 204), (status, body)
    for start in range(0, ENTITIES, 1000):
        batch = [entity(n, 0) for n in range(start, min(start + 1000, ENTITIES))]
        status, body = call("POST", "/ngsi-ld/v1/entityOperations/upsert", batch, timeout=300)
        assert status in (201, 204), (status, body[:300])
    for k in range(SUBS):
        sub = {
            "id": f"urn:ngsi-ld:Subscription:oomrig-{k}", "type": "Subscription",
            "entities": [{"type": TYPE}],
            "notification": {"endpoint": {"uri": f"http://127.0.0.1:{SINK_PORT}/n{k}", "accept": "application/json"}},
        }
        status, body = call("POST", "/ngsi-ld/v1/subscriptions", sub)
        assert status in (201, 409), (status, body)


def memory():
    out = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{.MemUsage}}", CONTAINER],
                         capture_output=True, text=True).stdout.split("/")[0].strip()
    for unit, scale in (("GiB", 1024), ("MiB", 1), ("KiB", 1 / 1024), ("B", 1 / 1048576)):
        if out.endswith(unit):
            return float(out[: -len(unit)]) * scale
    return 0.0


def alive():
    out = subprocess.run(["docker", "inspect", "-f", "{{.State.Running}} {{.State.OOMKilled}}", CONTAINER],
                         capture_output=True, text=True).stdout.split()
    return out == ["true", "false"], out


def main():
    server = ThreadingHTTPServer(("127.0.0.1", SINK_PORT), Slow)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    setup()
    stop = time.monotonic() + SECONDS
    reads, upserts, failures = [], [0], [0]

    def reader():
        while time.monotonic() < stop:
            for offset in range(0, ENTITIES, 100):
                if time.monotonic() >= stop:
                    return
                t = time.monotonic()
                status, _ = call("GET", f"/x/views/{TENANT}/ngsi-ld/v1/entities?type={TYPE}&limit=100&offset={offset}&count=true")
                reads.append(time.monotonic() - t)
                if status != 200:
                    failures[0] += 1

    def flooder():
        version = 1
        while time.monotonic() < stop:
            t = time.monotonic()
            start = random.randrange(0, max(1, ENTITIES - BATCH))
            status, _ = call("POST", "/ngsi-ld/v1/entityOperations/upsert?options=update",
                             [entity(n, version) for n in range(start, start + BATCH)])
            version += 1
            upserts[0] += 1
            if status not in (201, 204):
                failures[0] += 1
            time.sleep(max(0.0, FLOODERS / FLOOD - (time.monotonic() - t)))

    def discoverer():
        while time.monotonic() < stop:
            for what in ("types", "attributes"):
                status, _ = call("GET", f"/x/views/{TENANT}/ngsi-ld/v1/{what}?details=true", timeout=120)
                if status != 200:
                    failures[0] += 1

    threads = [threading.Thread(target=reader, daemon=True) for _ in range(READERS)]
    threads += [threading.Thread(target=discoverer, daemon=True) for _ in range(DISCOVERERS)]
    threads += [threading.Thread(target=flooder, daemon=True) for _ in range(FLOODERS)]
    for t in threads:
        t.start()
    peak, died = 0.0, None
    while time.monotonic() < stop:
        peak = max(peak, memory())
        ok, state = alive()
        if not ok:
            died = state
            break
        time.sleep(1)
    p95 = statistics.quantiles(reads, n=20)[18] if len(reads) >= 20 else float("nan")
    print(json.dumps({"peakMiB": round(peak), "died": died, "reads": len(reads), "readP95s": round(p95, 3),
                      "upserts": upserts[0], "failures": failures[0], "subs": SUBS, "seconds": SECONDS}))
    sys.exit(1 if died else 0)


if __name__ == "__main__":
    main()
