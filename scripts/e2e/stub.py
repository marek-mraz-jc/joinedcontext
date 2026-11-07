#!/usr/bin/env python3
"""Replay stub for scripts/e2e-parity.sh: every HTTP call of a parity run lands here.

Paths are /<run>/<route>, where <run> names one runner's run of one pipeline:
  /<run>/up/<scheme>/<host>/<path>  an outside source: served from the fixture store; in --record
                                    mode a miss is fetched from the real host once and stored.
  /<run>/token                      the OAuth2 token endpoint: a fixed bearer token.
  /<run>/gw/<path>                  the context gateway: upserts answer 204, reads an empty list.
Every request is appended to <log dir>/<run>.jsonl (method, route, query, body), so the two runners'
calls can be compared, and each fixture an outside call used to <log dir>/<run>.fixtures. Binds 127.0.0.1 only.

Usage: stub.py --port N --fixtures DIR --log DIR [--record]
"""
import argparse
import base64
import hashlib
import json
import os
import threading
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_BODY = 64 * 1024 * 1024
# Request headers that change what a source answers, so they are part of a fixture's key.
KEYED = ("range", "accept")
LOCK = threading.Lock()


def fixture_key(method, url, headers, body):
    h = hashlib.sha256()
    h.update(method.encode() + b"\n" + url.encode() + b"\n")
    for name in KEYED:
        value = headers.get(name, "")
        # reqwest sends `Accept: */*` where Go's client sends none; RFC 9110 reads both as any.
        if name == "accept" and value == "*/*":
            value = ""
        h.update(f"{name}:{value}\n".encode())
    h.update(body)
    return h.hexdigest()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    # Headers and body go out as two writes: without TCP_NODELAY the body waits on the client's
    # delayed ACK, 40 ms a request, which held a 2 000-call pipeline past the run's cut.
    disable_nagle_algorithm = True

    def log_message(self, *_):
        pass

    def answer(self, status, body=b"", content_type="application/json", extra=None):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def handle_any(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length > MAX_BODY:
            return self.answer(413)
        body = self.rfile.read(length) if length else b""
        path, _, query = self.path.partition("?")
        parts = path.lstrip("/").split("/", 1)
        if len(parts) != 2 or not parts[0].replace("-", "").replace("_", "").isalnum():
            return self.answer(404)
        run, route = parts
        try:
            text = body.decode("utf-8") if body else ""
            logged = json.loads(text) if text[:1] in "[{" else text
        except (UnicodeDecodeError, json.JSONDecodeError):
            logged = {"base64": base64.b64encode(body).decode()}
        record = {"method": self.command, "route": route, "query": query, "body": logged}
        for name in ("content-type", "range"):
            if name in self.headers:
                record[name] = self.headers[name]
        with LOCK, open(os.path.join(self.server.logs, run + ".jsonl"), "a") as log:
            log.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")

        if route == "token":
            return self.answer(200, b'{"access_token":"stub","token_type":"bearer","expires_in":3600}')
        if route.startswith("gw/"):
            if "entityOperations/upsert" in route:
                return self.answer(204)
            return self.answer(200, b"[]")
        if not route.startswith("up/"):
            return self.answer(404)
        scheme, _, rest = route[3:].partition("/")
        url = f"{scheme}://{rest}" + (f"?{query}" if query else "")
        headers = {k.lower(): v for k, v in self.headers.items()}
        key = fixture_key(self.command, url, headers, body)
        stored = os.path.join(self.server.fixtures, key[:2], key + ".json")
        with LOCK, open(os.path.join(self.server.logs, run + ".fixtures"), "a") as used:
            used.write(key + "\n")
        if not os.path.exists(stored):
            if not self.server.record:
                return self.answer(599, b"no recorded answer", "text/plain")
            self.fetch(url, headers, body, stored)
        with open(stored) as f:
            fixture = json.load(f)
        payload = base64.b64decode(fixture["body"])
        self.answer(fixture["status"], payload, fixture.get("content_type", "application/octet-stream"))

    def fetch(self, url, headers, body, stored):
        out = {k: headers[k] for k in KEYED if k in headers}
        out["accept-encoding"] = "identity"
        out["user-agent"] = "ekiben-parity-recorder"
        if "content-type" in headers:
            out["content-type"] = headers["content-type"]
        req = urllib.request.Request(url, data=body or None, method=self.command, headers=out)
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                status, ctype, payload = resp.status, resp.headers.get("Content-Type", ""), resp.read(MAX_BODY + 1)
        except urllib.error.HTTPError as err:
            status, ctype, payload = err.code, err.headers.get("Content-Type", ""), err.read(MAX_BODY + 1)
        except (urllib.error.URLError, TimeoutError, OSError) as err:
            status, ctype, payload = 598, "text/plain", f"recording failed: {err}".encode()
        if len(payload) > MAX_BODY:
            status, ctype, payload = 598, "text/plain", b"recording failed: body over 64 MiB"
        os.makedirs(os.path.dirname(stored), exist_ok=True)
        tmp = stored + f".{threading.get_ident()}"
        with open(tmp, "w") as f:
            json.dump({"url": url, "status": status, "content_type": ctype,
                       "body": base64.b64encode(payload).decode()}, f)
        os.replace(tmp, stored)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = handle_any


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--fixtures", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--record", action="store_true")
    args = ap.parse_args()
    os.makedirs(args.log, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    server.fixtures, server.logs, server.record = args.fixtures, args.log, args.record
    print(f"stub on 127.0.0.1:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
