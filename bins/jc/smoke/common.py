"""Shared parts of the jc smoke suites: a loopback-only HTTP client, the result rows and the
results.json + REPORT.md writer. Python 3 stdlib only (run with /opt/jc-py or any python3)."""

from __future__ import annotations

import base64
import http.client
import ipaddress
import json
import re
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

import live

PASS, FAIL, SKIP = "pass", "fail", "skip"

# jc's `[identity] realm` rule (setup::Realm::parse): a realm name goes into URL paths.
REALM_NAME = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9._-]*")


def realm_of(state) -> str:
    """The realm the rehearsal's jc runs on: `<state>/realm-name.txt` (deploy/rehearsal writes the
    export's realm there: dev, smartcity), else jc's default `jc`. A name jc would refuse is refused."""
    p = Path(state) / "realm-name.txt" if state else None
    name = p.read_text().strip() if p and p.is_file() else ""
    if not name:
        return "jc"
    if not REALM_NAME.fullmatch(name) or name.lower() == "master":
        raise SystemExit(f"{p}: {name!r} is no realm name jc accepts")
    return name


def loopback_only(base: str) -> None:
    """The suites run against a rehearsal on this host only, or in live mode (live.py) on the
    virtual base of the guard this process installed: never a bare dev or prod address."""
    host = urllib.parse.urlparse(base).hostname or ""
    if host == "localhost" or live.serves(base):
        return
    try:
        if ipaddress.ip_address(host).is_loopback:
            return
    except ValueError:
        pass
    raise SystemExit(f"refusing {host}: the smoke suites run against a local rehearsal only")


def browser_get(page, url: str, **kw):
    """`page.request.get` carrying the browser's own cookies for `url`. Chromium sends Secure and
    __Host- cookies on http://127.0.0.1 (a trustworthy origin), but Playwright's request client
    keeps them on http://localhost only: on the rehearsal's 127.0.0.1 origin `page.request` alone
    was signed out (/api/v1/auth/me 401 right after a good sign-in, jc-ip-signin 2026-11-10)."""
    u = urllib.parse.urlsplit(url)
    host, path = u.hostname or "", u.path or "/"
    sent = [f"{c['name']}={c['value']}" for c in page.context.cookies()
            if c["domain"].lstrip(".") == host and path.startswith(c["path"])]
    headers = dict(kw.pop("headers", None) or {})
    if sent and u.scheme == "http":
        headers.setdefault("Cookie", "; ".join(sent))
    return page.request.get(url, headers=headers, **kw)


def read_token(path: str | None) -> str | None:
    if not path:
        return None
    return Path(path).read_text().strip() or None


def jwt_claims(token: str | None) -> dict:
    """The payload of a JWT, unverified: only to pick which answer a check expects."""
    if not token or token.count(".") != 2:
        return {}
    part = token.split(".")[1]
    try:
        return json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))
    except ValueError:
        return {}


class Http:
    """Never follows redirects; answers (status, headers with lower-case names, body bytes)."""

    def __init__(self, base: str, timeout: float = 60):
        self.base = base.rstrip("/")
        self.timeout = timeout

        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *a, **k):
                return None

        self.opener = urllib.request.build_opener(NoRedirect)

    def call(self, method: str, path: str, token: str | None = None, headers: dict | None = None,
             body: bytes | dict | None = None):
        headers = dict(headers or {})
        if token:
            headers["Authorization"] = f"Bearer {token}"
        data = None
        if body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers.setdefault("Content-Type", "application/json")
        req = urllib.request.Request(self.base + path, data=data, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=self.timeout) as r:
                return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
        except urllib.error.HTTPError as e:
            return e.code, {k.lower(): v for k, v in (e.headers or {}).items()}, e.read() or b""
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            return 0, {}, str(e).encode()
        except http.client.HTTPException as e:
            # A body cut mid-read (IncompleteRead) is a failed call, never a crash of the suite.
            return 0, {}, repr(e).encode()


def as_json(raw: bytes):
    try:
        return json.loads(raw) if raw else None
    except ValueError:
        return None


def short(raw: bytes, n: int = 160) -> str:
    return raw[:n].decode(errors="replace").replace("\n", " ")


class Results:
    """One row per check: id, target, call, expected, actual, result (pass/fail/skip), reason."""

    def __init__(self, suite: str, base: str):
        self.suite, self.base = suite, base
        self.rows: list[dict] = []
        self.lock = threading.Lock()
        self.started = time.time()

    def add(self, id: str, target: str, call: str, expected: str, actual: str, result: str,
            reason: str = ""):
        assert result in (PASS, FAIL, SKIP)
        with self.lock:
            self.rows.append({"id": id, "target": target, "call": call, "expected": expected,
                              "actual": actual, "result": result, "reason": reason})

    def check(self, id, target, call, expected, actual, ok: bool, reason: str = ""):
        self.add(id, target, call, expected, actual, PASS if ok else FAIL, reason)

    def skip(self, id, target, call, expected, reason):
        self.add(id, target, call, expected, "", SKIP, reason)

    def totals(self) -> dict:
        out = {PASS: 0, FAIL: 0, SKIP: 0}
        for r in self.rows:
            out[r["result"]] += 1
        return out

    def write(self, out: Path, meta: dict | None = None) -> None:
        out.mkdir(parents=True, exist_ok=True)
        rows = sorted(self.rows, key=lambda r: (r["target"], r["id"]))
        if live.active():  # a live deployment's answers hold real people: kept redacted
            rows = [{**r, "actual": live.redact(r["actual"]), "reason": live.redact(r["reason"])} for r in rows]
        doc = {"suite": self.suite, "base": self.base, "seconds": round(time.time() - self.started),
               "totals": self.totals(), **(meta or {}), "checks": rows}
        (out / "results.json").write_text(json.dumps(doc, indent=1, ensure_ascii=False))
        t = self.totals()
        lines = [f"# Smoke suite `{self.suite}`", "",
                 f"Base {self.base}; {t[PASS]} pass, {t[FAIL]} fail, {t[SKIP]} skip "
                 f"({doc['seconds']} s).", ""]
        for k, v in (meta or {}).items():
            lines.append(f"- {k}: {v}")
        if meta:
            lines.append("")

        def cell(s: str) -> str:
            return str(s).replace("|", "\\|").replace("\n", " ")[:240]

        for title, which in (("Failures", FAIL), ("Skipped", SKIP), ("Passed", PASS)):
            chosen = [r for r in rows if r["result"] == which]
            lines += [f"## {title} ({len(chosen)})", ""]
            if not chosen:
                lines += ["None.", ""]
                continue
            lines += ["| id | target | call | expected | actual | reason |", "|---|---|---|---|---|---|"]
            for r in chosen:
                lines.append("| " + " | ".join(cell(r[k]) for k in
                                               ("id", "target", "call", "expected", "actual", "reason")) + " |")
            lines.append("")
        (out / "REPORT.md").write_text("\n".join(lines))
