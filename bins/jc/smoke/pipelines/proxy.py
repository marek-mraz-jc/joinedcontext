"""The counting proxy of the pipelines bench: one loopback port standing in for everything a
rendered stream calls, so every request it makes is counted.

Paths are /<run>/<route>, the scheme of crates/ekiben/scripts/e2e/stub.py (parity.prepare
rewrites a stream to it):
  /<run>/up/<scheme>/<host>/<path>   an outside source, answered from the recorded fixtures
                                     (a miss answers 599: nothing leaves the host)
  /<run>/token                       the OAuth2 token endpoint: a fixed bearer token
  /<run>/gw/api/endpoint/<slug>/ngsi-ld/v1/<rest>
                                     the context gateway: forwarded to Antares
                                     (<broker>/ngsi-ld/v1/<rest>) with the admin token and
                                     NGSILD-Tenant = bench-<run>
  /<run>/internal/<path>             the Portal's internal routes (outcomes, samples): 200 {}

Every request is one row in memory: run, route class, method, status, entities in the body,
body bytes, and the time the forward took (`upstream_ms`, the broker alone) beside the whole
answer (`ms`). Binds 127.0.0.1 only."""

from __future__ import annotations

import http.client
import json
import os
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MAX_BODY = 64 * 1024 * 1024


def _fixture_key():
    """crates/ekiben/scripts/e2e/stub.py fixture_key, the key the fixtures were recorded under."""
    here = Path(__file__).resolve()
    stub_dir = here.parents[4] / "crates" / "ekiben" / "scripts" / "e2e"
    sys.path.insert(0, str(stub_dir))
    from stub import fixture_key  # noqa: E402

    return fixture_key


def tenant_of(run: str) -> str:
    """One tenant per run, so each run's count starts at zero."""
    return "bench-" + run


def entities_in(route: str, body: bytes) -> int | None:
    """How many entities a batch request carries: the array length of an upsert/update/create/
    delete body, the id list of a batch query; None for any other call."""
    if "entityOperations/" not in route or not body:
        return None
    try:
        doc = json.loads(body)
    except ValueError:
        return None
    if isinstance(doc, list):
        return len(doc)
    if isinstance(doc, dict):
        ids = 0
        for sel in doc.get("entities") or []:
            i = sel.get("id")
            ids += len(i) if isinstance(i, list) else (1 if i else 0)
        return ids
    return None


class Proxy:
    def __init__(self, broker: str, admin_token: str, fixtures: str | None, port: int = 0,
                 delay_ms: float = 0, fail_every: int = 0):
        self.broker = urllib.parse.urlparse(broker.rstrip("/"))
        self.admin_token = admin_token
        self.fixtures = fixtures
        self.rows: list[dict] = []
        self.lock = threading.Lock()
        self.local = threading.local()
        self.fixture_key = _fixture_key() if fixtures else None
        proxy = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            disable_nagle_algorithm = True

            def log_message(self, *_):
                pass

            def answer(self, status, body=b"", ctype="application/json", extra=None):
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                for k, v in (extra or {}).items():
                    self.send_header(k, v)
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)

            def handle_any(self):
                t0 = time.perf_counter()
                length = int(self.headers.get("Content-Length") or 0)
                if length > MAX_BODY:
                    return self.answer(413)
                body = self.rfile.read(length) if length else b""
                path, _, query = self.path.partition("?")
                parts = path.lstrip("/").split("/", 1)
                if len(parts) != 2:
                    return self.answer(404)
                run, route = parts
                row = {"run": run, "method": self.command, "route": route, "query": query,
                       "bytes": len(body), "t0": time.time()}
                status, upstream_ms = proxy.dispatch(self, run, route, query, body, row)
                row.update(status=status, ms=round((time.perf_counter() - t0) * 1000, 2),
                           upstream_ms=upstream_ms)
                with proxy.lock:
                    proxy.rows.append(row)

            do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = handle_any

        self.server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
        self.server.daemon_threads = True
        self.port = self.server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}"
        self.delay_ms = delay_ms
        self.fail_every = fail_every
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def start(self) -> "Proxy":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.server.shutdown()

    def take(self, run: str | None = None) -> list[dict]:
        with self.lock:
            rows = [r for r in self.rows if run is None or r["run"] == run]
        return rows

    # --------------------------------------------------------------------------- routes

    def dispatch(self, h, run, route, query, body, row):
        if route == "token":
            row["class"] = "token"
            h.answer(200, b'{"access_token":"bench","token_type":"bearer","expires_in":3600}')
            return 200, None
        if route.startswith("internal/"):
            row["class"] = "internal:" + route.rsplit("/", 1)[-1]
            if route.endswith("/outcomes"):
                try:
                    doc = json.loads(body)
                    row["sent"] = len(doc.get("sent") or [])
                    row["failed"] = len(doc.get("failed") or [])
                except (ValueError, AttributeError):
                    pass
            h.answer(200, b"{}")
            return 200, None
        if route.startswith("gw/"):
            return self.forward(h, run, route[3:], query, body, row)
        if route.startswith("up/"):
            row["class"] = "source"
            return self.source(h, route[3:], query, body), None
        row["class"] = "unknown"
        h.answer(404)
        return 404, None

    def source(self, h, rest, query, body):
        if not self.fixtures:
            h.answer(599, b"no fixtures", "text/plain")
            return 599
        scheme, _, rest = rest.partition("/")
        url = f"{scheme}://{rest}" + (f"?{query}" if query else "")
        headers = {k.lower(): v for k, v in h.headers.items()}
        key = self.fixture_key(h.command, url, headers, body)
        stored = os.path.join(self.fixtures, key[:2], key + ".json")
        if not os.path.exists(stored):
            h.answer(599, b"no recorded answer", "text/plain")
            return 599
        with open(stored) as f:
            fixture = json.load(f)
        import base64

        payload = base64.b64decode(fixture["body"])
        h.answer(fixture["status"], payload, fixture.get("content_type", "application/octet-stream"))
        return fixture["status"]

    def _conn(self):
        c = getattr(self.local, "conn", None)
        if c is None:
            c = http.client.HTTPConnection(self.broker.hostname, self.broker.port, timeout=120)
            self.local.conn = c
        return c

    def forward(self, h, run, rest, query, body, row):
        # api/endpoint/<slug>/ngsi-ld/v1/<rest>
        p = rest.split("/", 3)
        if len(p) < 4 or p[0] != "api" or p[1] != "endpoint" or not p[3].startswith("ngsi-ld/v1/"):
            row["class"] = "gw:unknown"
            h.answer(404)
            return 404, None
        slug, tail = p[2], p[3][len("ngsi-ld/v1/"):]
        op = tail.split("?")[0]
        row["class"] = "gw:" + op
        row["slug"] = slug
        row["entities"] = entities_in(tail, body)
        self.counter = getattr(self, "counter", 0) + 1
        if self.fail_every and op.startswith("entityOperations/") and self.counter % self.fail_every == 0:
            h.answer(503, b'{"title":"bench: injected 503"}', "application/problem+json",
                     {"Retry-After": "1"})
            return 503, None
        if self.delay_ms:
            time.sleep(self.delay_ms / 1000)
        headers = {"Authorization": f"Bearer {self.admin_token}", "NGSILD-Tenant": tenant_of(run),
                   "Accept": h.headers.get("Accept") or "application/json"}
        if body:
            headers["Content-Type"] = h.headers.get("Content-Type") or "application/json"
        target = f"{self.broker.path}/ngsi-ld/v1/{tail}" + (f"?{query}" if query else "")
        t0 = time.perf_counter()
        for attempt in (0, 1):
            try:
                c = self._conn()
                c.request(h.command, target, body=body or None, headers=headers)
                r = c.getresponse()
                data = r.read()
                break
            except (http.client.HTTPException, OSError):
                self.local.conn = None
                if attempt:
                    h.answer(502, b'{"title":"bench: broker unreachable"}')
                    return 502, None
        upstream_ms = round((time.perf_counter() - t0) * 1000, 2)
        extra = {k: v for k, v in r.getheaders() if k.lower() in ("ngsild-results-count", "link")}
        h.answer(r.status, data, r.getheader("Content-Type") or "application/json", extra)
        return r.status, upstream_ms
