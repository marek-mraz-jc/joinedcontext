#!/usr/bin/env python3
"""The NLstn OData compliance suite's `Reseed` action in front of a broker (T-3714).

The suite (CONTRACT.md, v1) starts every suite with `POST {root}/Reseed` and fails the suite
unless it answers 200. The OData surface serves no action (ADR-0024), so this harness answers
that one request itself: it deletes every Entity of the view's types in the Tenant and writes
`ci/odata-compliance/seed.json` again, through the NGSI-LD API. Every other request goes to
the broker as it came (method, path, headers including `Host`, body) and its answer comes back
as it went, so what the suite measures is the broker.

    dev/odata-compliance/reseed-proxy.py LISTEN_PORT BROKER_URL ROOT_PATH

ROOT_PATH is the OData service root the suite is pointed at (`/x/views/compliance/odata/v4`).
Test harness only: it binds 127.0.0.1 and never runs beside a production broker.
"""

import http.client
import http.server
import json
import pathlib
import sys
import urllib.parse

HERE = pathlib.Path(__file__).resolve().parent
CI = HERE.parent.parent / "ci" / "odata-compliance"
TENANT = json.loads((CI / "view.json").read_text())["tenant"]
TYPES = json.loads((CI / "view.json").read_text())["selector"]
SEED = (CI / "seed.json").read_bytes()
HOP = {"connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade", "te", "trailer"}


def ngsi(broker, method, path, body=None):
    """One NGSI-LD call in the suite's Tenant: (status, parsed body or None)."""
    conn = http.client.HTTPConnection(broker.hostname, broker.port, timeout=30)
    headers = {"NGSILD-Tenant": TENANT}
    if body is not None:
        headers["Content-Type"] = "application/json"
    conn.request(method, path, body=body, headers=headers)
    resp = conn.getresponse()
    raw = resp.read()
    conn.close()
    return resp.status, (json.loads(raw) if raw else None)


def reseed(broker):
    """Every Entity of the view's types deleted, then the seed written; a refusal is an error."""
    for ty in TYPES:
        while True:
            status, found = ngsi(broker, "GET", f"/ngsi-ld/v1/entities?type={ty}&limit=1000")
            if status == 404:  # the Tenant does not exist before its first write
                break
            if status != 200:
                raise RuntimeError(f"query {ty}: HTTP {status}")
            ids = [e["id"] for e in found or []]
            if not ids:
                break
            status, _ = ngsi(broker, "POST", "/ngsi-ld/v1/entityOperations/delete", json.dumps(ids))
            if status not in (204, 207):
                raise RuntimeError(f"delete {ty}: HTTP {status}")
    status, _ = ngsi(broker, "POST", "/ngsi-ld/v1/entityOperations/create", SEED)
    if status != 201:
        raise RuntimeError(f"seed: HTTP {status}")


def handler(broker, root):
    class Proxy(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def answer(self, status, body, content_type="application/json"):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def forward(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else None
            if self.command == "POST" and urllib.parse.urlsplit(self.path).path == f"{root}/Reseed":
                try:
                    reseed(broker)
                except (OSError, RuntimeError, ValueError) as e:
                    return self.answer(500, json.dumps({"error": str(e)}).encode())
                return self.answer(200, b'{"status":"success"}')
            conn = http.client.HTTPConnection(broker.hostname, broker.port, timeout=60)
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
            conn.request(self.command, self.path, body=body, headers=headers)
            resp = conn.getresponse()
            data = resp.read()
            self.send_response_only(resp.status, resp.reason)
            for k, v in resp.getheaders():
                if k.lower() not in HOP and k.lower() != "content-length":
                    self.send_header(k, v)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(data)
            conn.close()

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = forward

    return Proxy


def main():
    port, broker, root = int(sys.argv[1]), urllib.parse.urlsplit(sys.argv[2]), sys.argv[3].rstrip("/")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler(broker, root))
    server.serve_forever()


if __name__ == "__main__":
    main()
