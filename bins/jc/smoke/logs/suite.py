"""Smoke suite `logs`: what jc's log, metrics and activity feed say about a run.

    bins/jc/smoke/run logs --base-url http://localhost:<port> --log-file <home>/jc.log \
        --state <home>/state [--secrets-dir <home>/secrets] [--admin-url http://127.0.0.1:<admin>] \
        [--admin-token-file <home>/secrets/metrics-token] [--results-dir <other suite out>]... --out <dir>

Checks, one row each (results.json, REPORT.md):
- refused.<module>: one refused call per module door (no or a wrong credential) with a fresh
  X-Request-Id: the status, the id echoed, and exactly one log line carrying the id with a reason.
- rid.flow.<flow>: a call that crosses modules (Portal -> gateway -> Antares, Portal -> shako,
  gateway -> Antares, ekiben -> broker): every module on the way logs the same request id.
- log.<module>: the module wrote lines, every line parses (text or JSON format).
- metrics.<module>: jc's admin /metrics carries the module's series; the admin scrape refuses a
  caller without its token; the public listener serves no scrape.
- activity.*: the Portal activity feed answers per project and holds events in the windows the
  other suites' runs (--results-dir) wrote in.
- secrets.* / personal.*: no secret value of --state / --secrets-dir (raw, URL- or base64-encoded),
  no JWT, bearer value or password field, and no e-mail or public IP address in the log or in any
  file of the other suites' results.
- volume.<module>: lines and bytes per minute while nobody calls jc (a --quiet-window the suite waits
  out at its end, its own probes left out), against --volume-budget lines per minute: the polls and
  loops that run whatever the traffic. The whole log's rate, which grows with the other suites'
  load, is in `actual` for reference.
- volume.request: lines one request id writes across all modules (p95 over the whole log), against
  --lines-per-request: prod's stack writes 3 for an Endpoint read (APISIX's access line, the
  gateway's forward line, the broker's surface operation).

The person the feed is read with is made in realm `jc` (group /platform-admins) with a random
password kept in memory, and deleted at the end."""

from __future__ import annotations

import argparse
import datetime as dt
import html
import json
import re
import secrets
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import logscan as ls  # noqa: E402
from common import PASS, FAIL, Results, as_json, loopback_only, read_token, realm_of, short  # noqa: E402

PORTAL_ADMIN_CLIENT = "portal-reconciler"
PERSON = "smoke-logs-reader"
REASON_WORDS = re.compile(r"(?i)refus|denied|deny|unauthori|forbidden|not allowed|invalid|reject|"
                          r"401|403|no token|missing|bad credential|without")
# The metric name prefixes each module records under.
METRIC_PREFIXES = {
    "portal": ("jc_portal_", "jc_agent_run_"), "gateway": ("jc_gateway_",), "antares": ("antares_",),
    "ekiben": ("ekiben_",), "shako": ("shako_",), "kaisatsu": ("kaisatsu_",), "daiya": ("daiya_",),
    "dataroom": ("dataroom_",),
}
MUTATING = re.compile(r"^(POST|PUT|PATCH|DELETE)\b|tools/call\s+\S*(propose|create|delete|revoke|update|apply)",
                      re.I)


def new_id() -> str:
    return "smokelogs-" + secrets.token_hex(12)


def iso(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class Session:
    """HTTP with a cookie jar (Secure cookies kept on http://localhost, as browsers do); never
    follows redirects by itself. Answers (status, lower-case headers, body)."""

    def __init__(self, base: str, timeout: float = 60):
        self.base, self.timeout = base.rstrip("/"), timeout
        self.jar: dict[str, tuple[str, str]] = {}

        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *a, **k):
                return None

        self.opener = urllib.request.build_opener(NoRedirect)

    def url(self, path: str) -> str:
        return path if path.startswith("http") else self.base + path

    def _store(self, headers):
        for raw in headers.get_all("Set-Cookie") or []:
            parts = [p.strip() for p in raw.split(";")]
            if "=" not in parts[0]:
                continue
            name, value = parts[0].split("=", 1)
            path = next((a[5:] for a in parts[1:] if a.lower().startswith("path=")), "/")
            if value and "max-age=0" not in (a.lower() for a in parts[1:]):
                self.jar[name] = (value, path)
            else:
                self.jar.pop(name, None)

    def call(self, method, path, headers=None, body=None, form=None, cookies=True):
        url = self.url(path)
        loopback_only(url)
        headers = dict(headers or {})
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode()
            headers.setdefault("Content-Type", "application/x-www-form-urlencoded")
        elif body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers.setdefault("Content-Type", "application/json")
        if cookies:
            p = urllib.parse.urlparse(url).path or "/"
            c = "; ".join(f"{n}={v}" for n, (v, cp) in self.jar.items() if p.startswith(cp))
            if c:
                headers.setdefault("Cookie", c)
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=self.timeout) as r:
                self._store(r.headers)
                return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
        except urllib.error.HTTPError as e:
            if e.headers and cookies:
                self._store(e.headers)
            return e.code, {k.lower(): v for k, v in (e.headers or {}).items()}, e.read() or b""
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            return 0, {}, str(e).encode()

    def follow(self, path, hops=12):
        url = self.url(path)
        for _ in range(hops):
            st, h, body = self.call("GET", url)
            if st in (301, 302, 303, 307, 308) and h.get("location"):
                url = urllib.parse.urljoin(url, h["location"])
                continue
            return url, st, body
        raise RuntimeError(f"too many redirects from {path}")


def hidden(page: str, name: str) -> str:
    m = re.search(rf'<input[^>]*name="{name}"[^>]*>', page)
    v = re.search(r'value="([^"]*)"', m.group(0)) if m else None
    if not v:
        raise RuntimeError(f"no {name} on the login page")
    return html.unescape(v.group(1))


class Person:
    """A platform admin of realm `jc` for the feed reads: made, signed in, deleted."""

    def __init__(self, base: str, state: Path):
        self.base, self.state = base, state
        self.realm = realm_of(state)
        self.uid = None
        self.session = Session(base)
        self.admin = None

    def _admin_token(self) -> str:
        secret = (self.state / "portal-admin-client-secret").read_text().strip()
        st, _, raw = Session(self.base).call(
            "POST", f"/id/realms/{self.realm}/protocol/openid-connect/token", cookies=False,
            form={"grant_type": "client_credentials", "client_id": PORTAL_ADMIN_CLIENT, "client_secret": secret})
        tok = as_json(raw) or {}
        if st != 200 or "access_token" not in tok:
            raise RuntimeError(f"{PORTAL_ADMIN_CLIENT} token: {st}")
        return tok["access_token"]

    def sign_in(self) -> None:
        self.admin = self._admin_token()
        auth = {"Authorization": f"Bearer {self.admin}"}
        s = Session(self.base)
        password = secrets.token_urlsafe(18) + "-7aZ"
        st, _, raw = s.call("POST", f"/id/admin/realms/{self.realm}/users", headers=auth, cookies=False, body={
            "username": PERSON, "enabled": True, "email": f"{PERSON}@smoke.invalid", "emailVerified": True,
            "firstName": "Smoke", "lastName": "Logs"})
        if st not in (201, 409):
            raise RuntimeError(f"create {PERSON}: {st} {short(raw)}")
        st, _, raw = s.call("GET", f"/id/admin/realms/{self.realm}/users?username={PERSON}&exact=true", headers=auth,
                            cookies=False)
        self.uid = (as_json(raw) or [{}])[0].get("id")
        if not self.uid:
            raise RuntimeError(f"find {PERSON}: {st}")
        st, _, raw = s.call("PUT", f"/id/admin/realms/{self.realm}/users/{self.uid}/reset-password", headers=auth,
                            cookies=False, body={"type": "password", "value": password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of {PERSON}: {st}")
        st, _, raw = s.call("GET", f"/id/admin/realms/{self.realm}/groups?briefRepresentation=false", headers=auth,
                            cookies=False)
        todo, gid = list(as_json(raw) or []), None
        while todo:
            g = todo.pop()
            if g.get("path") == "/platform-admins":
                gid = g["id"]
            todo += g.get("subGroups") or []
        if not gid:
            raise RuntimeError(f"group /platform-admins: not in realm {self.realm}")
        st, _, _ = s.call("PUT", f"/id/admin/realms/{self.realm}/users/{self.uid}/groups/{gid}", headers=auth, cookies=False)
        if st not in (200, 204):
            raise RuntimeError(f"group of {PERSON}: {st}")
        login_url, st, page = self.session.follow("/api/v1/auth/login")
        if st != 200:
            raise RuntimeError(f"GET /api/v1/auth/login -> {st}")
        text = page.decode(errors="replace")
        st, h, raw = self.session.call("POST", login_url, form={
            "csrf_token": hidden(text, "csrf_token"), "session_code": hidden(text, "session_code"),
            "username": PERSON, "password": password})
        loc = h.get("location", "")
        if st not in (302, 303) or "code=" not in loc:
            raise RuntimeError(f"login form -> {st}")
        _, st, _ = self.session.follow(urllib.parse.urljoin(login_url, loc))
        if st >= 400:
            raise RuntimeError(f"callback -> {st}")

    def delete(self) -> None:
        if self.uid and self.admin:
            Session(self.base).call("DELETE", f"/id/admin/realms/{self.realm}/users/{self.uid}", cookies=False,
                                    headers={"Authorization": f"Bearer {self.admin}"})


# ------------------------------------------------------------------ refused calls, one per door

def refused_calls(marker: str, slug: str | None = None, realm: str = "jc") -> list[dict]:
    """(module, method, path, headers, body, accepted statuses): each refused before any work.
    The gateway's call names a public Endpoint when one is known (a forged token is 401 there),
    else an unknown one (404, which says nothing more)."""
    bad = {"Authorization": "Bearer smokelogs.not.a.token"}
    return [
        {"module": "portal", "method": "GET", "path": "/api/v1/projects", "headers": {}, "ok": (401,)},
        {"module": "portal", "id": "portal.bad-token", "method": "GET", "path": "/api/v1/projects",
         "headers": bad, "ok": (401,)},
        {"module": "gateway", "method": "GET", "path": f"/api/endpoint/{slug or marker}/ngsi-ld/v1/types",
         "headers": bad, "ok": (401, 403) if slug else (401, 403, 404)},
        {"module": "antares", "method": "POST", "path": "/broker/ngsi-ld/v1/entities", "headers": {},
         "body": {"id": f"urn:ngsi-ld:SmokeLogs:{marker}", "type": "SmokeLogs"}, "ok": (401, 403)},
        {"module": "shako", "method": "GET", "path": "/git/api/v1/user", "headers": bad,
         "ok": (401, 403)},
        {"module": "ekiben", "method": "GET", "path": "/streams/streams", "headers": bad, "ok": (401, 403)},
        {"module": "kaisatsu", "method": "POST", "path": f"/id/realms/{realm}/protocol/openid-connect/token",
         "headers": {}, "form": {"grant_type": "client_credentials", "client_id": PORTAL_ADMIN_CLIENT,
                                 "client_secret": marker}, "ok": (400, 401)},
        {"module": "daiya", "method": "POST", "path": "/dashboards/api/folders", "headers": bad,
         "body": {"title": marker}, "ok": (401, 403, 404)},
        {"module": "dataroom", "method": "POST", "path": "/opendata/api/3/action/package_create",
         "headers": bad, "body": {"name": marker}, "ok": (401, 403)},
    ]


def lines_with(records, rid: str):
    return [r for r in records if r.request_id == rid or rid in r.raw]


def in_window(records, module: str, t0: float, t1: float):
    a = dt.datetime.fromtimestamp(t0 - 0.5, dt.timezone.utc)
    b = dt.datetime.fromtimestamp(t1 + 2, dt.timezone.utc)
    return [r for r in records if r.ts and a <= r.ts <= b and r.module == module]


def judge_refused(res: Results, probe: dict, records) -> None:
    m, pid = probe["module"], probe.get("id") or probe["module"]
    call = f"{probe['method']} {probe['path']} (X-Request-Id {probe['rid']})"
    res.check(f"refused.{pid}.status", m, call, f"one of {list(probe['ok'])}", str(probe["status"]),
              probe["status"] in probe["ok"], "" if probe["status"] else short(probe["body"]))
    if probe["status"] == 0:
        for what in ("echo", "logged", "reason"):
            res.skip(f"refused.{pid}.{what}", m, call, "", "the door did not answer")
        return
    echoed = probe["headers"].get("x-request-id", "")
    res.check(f"refused.{pid}.echo", m, call, "the answer carries the sent X-Request-Id",
              echoed or "(none)", echoed == probe["rid"],
              "" if echoed == probe["rid"] else "the module answers without the caller's request id")
    tagged = lines_with(records, probe["rid"])
    # One access line per door; the module may add lines of its own in the same request span.
    access = [r for r in tagged if r.target == "jc::access"] or tagged
    res.check(f"refused.{pid}.logged", m, call, "exactly one access line carries the request id",
              f"{len(access)} access line(s), {len(tagged)} with the id" +
              (f": {tagged[0].target}: {tagged[0].message[:80]}" if tagged else ""),
              len(access) == 1,
              "" if tagged else "no log line names the request id")
    if tagged:
        line = tagged[0]
        has_reason = bool(REASON_WORDS.search(line.raw)) or any(
            k in line.fields for k in ("reason", "error", "status"))
        res.check(f"refused.{pid}.reason", m, call, "the line says why (status, reason or error)",
                  line.raw[:200], has_reason)
    else:
        nearby = [r for r in in_window(records, m, probe["t0"], probe["t1"]) if REASON_WORDS.search(r.raw)]
        res.check(f"refused.{pid}.reason", m, call, "a line of the module says why the call was refused",
                  f"{len(nearby)} line(s) of {m} in the call's window" +
                  (f": {nearby[0].raw[:160]}" if nearby else ""),
                  len(nearby) == 1,
                  "logged without the request id" if nearby else "the refusal is not logged")


def run_probe(h: Session, probe: dict) -> dict:
    probe["rid"] = new_id()
    headers = {**probe["headers"], "X-Request-Id": probe["rid"]}
    probe["t0"] = time.time()
    st, hd, body = h.call(probe["method"], probe["path"], headers=headers, body=probe.get("body"),
                          form=probe.get("form"), cookies=False)
    probe["t1"] = time.time()
    probe.update(status=st, headers=hd, body=body)
    return probe


# ------------------------------------------------------------------ flows across modules

def judge_flow(res: Results, name: str, call: str, rid: str, expected: tuple, records, status: int,
               why_skip: str = "") -> None:
    if why_skip:
        res.skip(f"rid.flow.{name}", "+".join(expected), call, f"{', '.join(expected)} log {rid}", why_skip)
        return
    seen = sorted({r.module for r in lines_with(records, rid)})
    missing = [m for m in expected if m not in seen]
    res.check(f"rid.flow.{name}", "+".join(expected), call,
              f"lines of {', '.join(expected)} carry the request id",
              f"status {status}; modules with the id: {', '.join(seen) or 'none'}", not missing,
              f"no line of {', '.join(missing)} carries the id" if missing else "")


def public_slugs(h: Session) -> tuple[list[str], str]:
    """The public Endpoints the MCP hub lists anonymously, and what it answered."""
    st, _, raw = h.call("POST", "/api/mcp", cookies=False,
                        headers={"Accept": "application/json, text/event-stream"},
                        body={"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                              "params": {"name": "list_endpoints", "arguments": {}}})
    doc = as_json(raw) or {}
    eps = ((doc.get("result") or {}).get("structuredContent") or {}).get("endpoints") or []
    slugs = [e["slug"] for e in eps if isinstance(e, dict) and e.get("slug") and e.get("access", "public") == "public"]
    return slugs, f"{st} {len(eps)} endpoints"


# ------------------------------------------------------------------ metrics

def metric_names(text: str) -> set[str]:
    return {re.split(r"[{\s]", line, maxsplit=1)[0] for line in text.splitlines()
            if line and not line.startswith("#")}


def judge_metrics(res: Results, a) -> None:
    pub = Session(a.base_url)
    st, hd, body = pub.call("GET", "/metrics", cookies=False)
    res.check("metrics.public.closed", "jc", "GET /metrics on the public listener",
              "no scrape on the public listener (401, 403 or 404)", f"{st} {len(body)} B {hd.get('content-type', '')}",
              st in (401, 403, 404),
              "the public listener answers a Prometheus scrape to anyone" if st == 200 else "")
    if not a.admin_url:
        for m in METRIC_PREFIXES:
            res.skip(f"metrics.{m}", m, "GET <admin>/metrics", "the module's series", "no --admin-url")
        return
    adm = Session(a.admin_url)
    st, _, _ = adm.call("GET", "/metrics", cookies=False)
    res.check("metrics.admin.anonymous", "jc", f"GET {a.admin_url}/metrics (no token)", "401", str(st), st == 401)
    token = read_token(a.admin_token_file)
    if not token:
        for m in METRIC_PREFIXES:
            res.skip(f"metrics.{m}", m, "GET <admin>/metrics", "the module's series", "no --admin-token-file")
        return
    st, _, body = adm.call("GET", "/metrics", headers={"Authorization": f"Bearer {token}"}, cookies=False)
    res.check("metrics.admin.answers", "jc", f"GET {a.admin_url}/metrics (token)", "200 Prometheus text",
              f"{st} {len(body)} B", st == 200 and len(body) > 0)
    names = metric_names(body.decode(errors="replace")) if st == 200 else set()
    for m, prefixes in METRIC_PREFIXES.items():
        mine = sorted(n for n in names if n.startswith(prefixes))
        res.check(f"metrics.{m}", m, f"GET {a.admin_url}/metrics", f"series named {'|'.join(prefixes)}*",
                  f"{len(mine)} series" + (f" ({', '.join(mine[:4])}...)" if mine else ""), bool(mine),
                  "" if mine else "the module records no metric into jc's recorder")
    # The modules' own scrape doors, where they keep one.
    state = Path(a.state) if a.state else None
    own = [("ekiben", "/streams/metrics", "ekiben-token")]
    for m, path, tok_file in own:
        tok = (state / tok_file).read_text().strip() if state and (state / tok_file).is_file() else None
        if not tok:
            res.skip(f"metrics.{m}.own", m, f"GET {path}", "200", f"no {tok_file} in --state")
            continue
        st, _, body = pub.call("GET", path, headers={"Authorization": f"Bearer {tok}"}, cookies=False)
        res.check(f"metrics.{m}.own", m, f"GET {path} (module token)", "200 with series", f"{st} {len(body)} B",
                  st == 200 and len(metric_names(body.decode(errors="replace"))) > 0,
                  "answers no series" if st == 200 else "")


# ------------------------------------------------------------------ activity feed

def suite_windows(dirs: list[Path]) -> list[dict]:
    out = []
    for f in [f for d in dirs for f in sorted(d.rglob("results.json"))]:
        doc = as_json(f.read_bytes())
        if not isinstance(doc, dict) or not isinstance(doc.get("checks"), list):
            continue
        end = f.stat().st_mtime
        start = end - float(doc.get("seconds") or 0)
        writes = [c for c in doc.get("checks", []) if c.get("result") == PASS and MUTATING.search(str(c.get("call", "")))]
        out.append({"suite": doc.get("suite", f.parent.name), "dir": str(f.parent), "start": start - 60, "end": end + 60,
                    "writes": len(writes), "example": writes[0]["call"][:120] if writes else ""})
    return out


def judge_activity(res: Results, person: Person | None, why: str, windows: list[dict], since: dt.datetime):
    if person is None:
        res.skip("activity.feed", "portal", "GET /api/v1/projects/{p}/activity", "200 per project", why)
        for w in windows:
            res.skip(f"activity.{w['suite']}", "portal", "GET /api/v1/projects/{p}/activity", "events", why)
        return
    s = person.session
    st, _, raw = s.call("GET", "/api/v1/projects")
    doc = as_json(raw)
    rows = doc if isinstance(doc, list) else (doc or {}).get("items", []) if isinstance(doc, dict) else []
    projects = sorted({(p.get("name") or (p.get("metadata") or {}).get("name")) for p in rows if isinstance(p, dict)} - {None})
    res.check("activity.projects", "portal", "GET /api/v1/projects", "200 with the projects", f"{st} {len(projects)}",
              st == 200 and bool(projects))
    first = min([since] + [dt.datetime.fromtimestamp(w["start"], dt.timezone.utc) for w in windows])
    events, bad = [], []
    for p in projects:
        st, _, raw = s.call("GET", f"/api/v1/projects/{p}/activity?since={urllib.parse.quote(iso(first))}&limit=200")
        if st != 200:
            bad.append(f"{p}: {st}")
            continue
        for e in (as_json(raw) or {}).get("items", []):
            t = ls.parse_time(str(e.get("time", "")))
            if t:
                events.append((t, p, e.get("kind"), e.get("source")))
    res.check("activity.feed", "portal", "GET /api/v1/projects/{p}/activity?since=...", "200 for every project",
              f"{len(projects) - len(bad)}/{len(projects)} answered, {len(events)} events" +
              (f"; {', '.join(bad[:5])}" if bad else ""), not bad and bool(projects))
    for i, w in enumerate(windows, 1):
        w["suite"] = f"{w['suite']}.{i}"
        a = dt.datetime.fromtimestamp(w["start"], dt.timezone.utc)
        b = dt.datetime.fromtimestamp(w["end"], dt.timezone.utc)
        inside = [e for e in events if a <= e[0] <= b]
        call = f"activity of {iso(a)}..{iso(b)} (suite {w['suite']}, {w['writes']} passed writes)"
        if not w["writes"]:
            res.skip(f"activity.{w['suite']}", "portal", call, "events of the suite's writes",
                     "the suite's results name no passed write")
            continue
        kinds = sorted({str(e[2]) for e in inside})
        res.check(f"activity.{w['suite']}", "portal", call, "at least one event in the suite's window",
                  f"{len(inside)} events ({', '.join(kinds[:6])})", bool(inside),
                  "" if inside else f"no event for writes such as {w['example']}")


# ------------------------------------------------------------------ secrets and personal data

def scan_text_sources(res: Results, index: ls.SecretIndex, sources: list[tuple[str, str, Path]]):
    """sources: (check id, target, file)."""
    for cid, target, f in sources:
        try:
            text = f.read_text(errors="replace")
        except OSError as e:
            res.skip(f"secrets.{cid}", target, f"scan {f}", "no secret value", str(e))
            continue
        hits = index.find(text)
        res.check(f"secrets.{cid}", target, f"scan {f} for the setup secrets", "no secret value",
                  f"{len(hits)} secret(s): {', '.join(sorted(s.name for s in hits)[:8])}" if hits else "none",
                  not hits)
        shapes: dict[str, list[int]] = {}
        people: dict[str, list[int]] = {}
        for n, line in enumerate(text.splitlines(), 1):
            for k in ls.credential_shapes(line):
                shapes.setdefault(k, []).append(n)
            for p in ls.personal_data(line):
                people.setdefault(p, []).append(n)
        res.check(f"secrets.{cid}.shapes", target, f"scan {f} for JWTs, bearer values, password fields",
                  "none", "; ".join(f"{k} at line {v[0]} ({len(v)}x)" for k, v in shapes.items()) or "none",
                  not shapes)
        res.check(f"personal.{cid}", target, f"scan {f} for e-mail and public IP addresses",
                  "none beyond user ids",
                  "; ".join(f"{k} at line {v[0]} ({len(v)}x)" for k, v in list(people.items())[:8]) or "none",
                  not people)


# ------------------------------------------------------------------ main

def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--log-file")
    p.add_argument("--state", help="the rehearsal's state dir (tokens, the Portal admin client secret)")
    p.add_argument("--secrets-dir", action="append", default=[])
    p.add_argument("--admin-url", help="jc's admin listener, e.g. http://127.0.0.1:55641")
    p.add_argument("--admin-token-file")
    p.add_argument("--person-token-file", help="accepted for the runner's common flags; unused")
    p.add_argument("--results-dir", action="append", default=[], help="another suite's --out, to scan")
    p.add_argument("--volume-budget", type=float, default=600,
                   help="lines per minute per module while nobody calls jc")
    p.add_argument("--quiet-window", type=float, default=120,
                   help="seconds the suite waits at its end, with no calls, to measure --volume-budget")
    p.add_argument("--lines-per-request", type=int, default=3,
                   help="p95 lines one request id writes across modules (prod's stack: 3)")
    p.add_argument("--settle", type=float, default=3, help="seconds to wait for the log to catch up")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    if a.admin_url:
        loopback_only(a.admin_url)
    res = Results("logs", a.base_url)
    started = dt.datetime.now(dt.timezone.utc)
    log = Path(a.log_file) if a.log_file else None
    offset = log.stat().st_size if log and log.is_file() else 0
    h = Session(a.base_url)
    marker = "smokelogs" + secrets.token_hex(6)

    slugs, hub = public_slugs(h)
    realm = realm_of(a.state) if a.state else "jc"
    probes = [run_probe(h, pr) for pr in refused_calls(marker, slugs[0] if slugs else None, realm)]

    # Flows: each with its own request id.
    flows = []
    # A random one: the Portal keeps a standards answer for a while, and a kept answer asks nobody.
    slug = secrets.choice(slugs) if slugs else None
    rid = new_id()
    if slug:
        call = f"GET /api/v1/catalogue/endpoints/{slug}/standards"
        s, _, _ = h.call("GET", f"/api/v1/catalogue/endpoints/{slug}/standards", headers={"X-Request-Id": rid},
                         cookies=False)
        flows.append(("portal-gateway-antares", call, rid, ("portal", "gateway", "antares"), s, ""))
    else:
        flows.append(("portal-gateway-antares", "GET /api/v1/catalogue/endpoints/{slug}/standards", rid,
                      ("portal", "gateway", "antares"), 0, f"the MCP hub lists no public Endpoint ({hub})"))
    rid = new_id()
    if slug:
        call = f"GET /api/endpoint/{slug}/ngsi-ld/v1/types"
        s, _, _ = h.call("GET", f"/api/endpoint/{slug}/ngsi-ld/v1/types", headers={"X-Request-Id": rid},
                         cookies=False)
        flows.append(("gateway-antares", call, rid, ("gateway", "antares"), s, ""))
    else:
        flows.append(("gateway-antares", "GET /api/endpoint/{slug}/ngsi-ld/v1/types", rid,
                      ("gateway", "antares"), 0, f"the MCP hub lists no public Endpoint ({hub})"))

    person, why = None, ""
    if a.state:
        try:
            person = Person(a.base_url, Path(a.state))
            person.sign_in()
        except (RuntimeError, OSError, KeyError, IndexError) as e:
            why, person = f"no signed-in reader: {e}", None
    else:
        why = "no --state (the Portal admin client secret makes the reader)"
    rid = new_id()
    if person:
        st, _, raw = person.session.call("GET", "/api/v1/projects")
        doc = as_json(raw)
        rows = doc if isinstance(doc, list) else (doc or {}).get("items", []) if isinstance(doc, dict) else []
        proj = next((r.get("name") or (r.get("metadata") or {}).get("name") for r in rows if isinstance(r, dict)), None)
        if proj:
            call = f"GET /api/v1/projects/{proj}/revisions"
            s, _, _ = person.session.call("GET", f"/api/v1/projects/{proj}/revisions", headers={"X-Request-Id": rid})
            flows.append(("portal-shako", call, rid, ("portal", "shako"), s, ""))
        else:
            flows.append(("portal-shako", "GET /api/v1/projects/{p}/revisions", rid, ("portal", "shako"), st,
                          f"the Portal lists no project ({st})"))
    else:
        flows.append(("portal-shako", "GET /api/v1/projects/{p}/revisions", rid, ("portal", "shako"), 0, why))

    time.sleep(a.settle)
    if log and log.is_file():
        fresh, _ = ls.read_records(log, offset)
        for pr in probes:
            judge_refused(res, pr, fresh)
        for fl in flows:
            judge_flow(res, *fl[:4], fresh, fl[4], fl[5])
    else:
        for pr in probes:
            res.skip(f"refused.{pr.get('id') or pr['module']}", pr["module"], pr["path"], "", "no --log-file")
        for fl in flows:
            res.skip(f"rid.flow.{fl[0]}", "+".join(fl[3]), fl[1], "", "no --log-file")

    # ekiben -> broker: a stream run's writes reach the broker with the run's id (passive, whole log).
    records = ls.read_records(log)[0] if log and log.is_file() else []
    eki = [r for r in records if r.module == "ekiben" and r.request_id]
    ant = {r.request_id for r in records if r.module == "antares" and r.request_id}
    shared = [r for r in eki if r.request_id in ant]
    if not records:
        res.skip("rid.flow.ekiben-antares", "ekiben+antares", "stream runs in the log", "", "no --log-file")
    else:
        res.check("rid.flow.ekiben-antares", "ekiben+antares", "stream runs writing to the broker (whole log)",
                  "an ekiben line and an antares line share a request id",
                  f"{len(eki)} ekiben line(s) with an id, {len(shared)} shared with antares", bool(shared),
                  "" if shared else ("ekiben's lines carry no request id" if not eki
                                     else "the broker logs another id than ekiben's"))

    # Structured lines and log volume over the whole log.
    if records:
        per = {m: [r for r in records if r.module == m] for m in ls.MODULES}
        for m, rs in per.items():
            res.check(f"log.{m}.writes", m, f"lines of {m} in {log}", "at least one line", f"{len(rs)} lines",
                      bool(rs), "" if rs else "the module wrote no line")
        loose = [r for r in records if not r.structured]
        res.check("log.all.structured", "jc", f"parse every line of {log}", "every line is a structured record",
                  f"{len(records) - len(loose)}/{len(records)} parse" +
                  (f"; first loose at line(s) {', '.join(str(r.n) for r in loose[:5])}" if loose else ""), not loose)
        whole = ls.volume(records)
        quiet_from = log.stat().st_size
        time.sleep(a.quiet_window)
        quiet = [r for r in ls.read_records(log, quiet_from)[0]
                 if not (r.request_id or "").startswith("smokelogs-")]
        vol = ls.volume(quiet, minutes=a.quiet_window / 60)
        for m in sorted(set(whole) | set(vol)):
            v = vol.get(m) or {"linesPerMinute": 0, "bytesPerMinute": 0, "topMessage": "none", "topCount": 0}
            ok = v["linesPerMinute"] <= a.volume_budget
            w = whole.get(m, {}).get("linesPerMinute", 0)
            res.check(f"volume.{m}", m, f"lines of {m} in {a.quiet_window:g} s without calls",
                      f"<= {a.volume_budget:g} lines/min",
                      f"{v['linesPerMinute']} lines/min, {v['bytesPerMinute']} B/min; top: "
                      f"{v['topMessage'][:90]} ({v['topCount']}x); whole log under load {w} lines/min", ok,
                      "" if ok else "over the budget")
        per = ls.request_lines(records, a.lines_per_request)
        ok = per["p95"] <= a.lines_per_request
        res.check("volume.request", "jc", f"lines per request id across modules ({per['requests']} requests)",
                  f"p95 <= {a.lines_per_request}",
                  f"p95 {per['p95']}, mean {per['mean']}, max {per['max']}; {per['over']} over "
                  f"{a.lines_per_request}" + (f", most often: {per['shape'][:200]}" if per["shape"] else ""), ok,
                  "" if ok else "a request writes more lines than prod's stack")
    else:
        res.skip("log.all.structured", "jc", "parse the log", "", "no --log-file")

    judge_metrics(res, a)

    windows = suite_windows([Path(d) for d in a.results_dir])
    judge_activity(res, person, why, windows, started - dt.timedelta(minutes=5))

    # Secrets and personal data: the log and every text file of the other suites' results.
    dirs = [Path(d) for d in a.secrets_dir]
    if a.state:
        dirs.append(Path(a.state))
        if not a.secrets_dir and (Path(a.state).parent / "secrets").is_dir():
            dirs.append(Path(a.state).parent / "secrets")
    found = ls.collect_secrets(dirs)
    if not found:
        res.skip("secrets.collect", "jc", "read --state / --secrets-dir", "secret values to look for",
                 "no secret file found")
    else:
        res.add("secrets.collect", "jc", f"read {', '.join(str(d) for d in dirs)}", "secret values to look for",
                f"{len(found)} values", PASS)
        index = ls.SecretIndex(found)
        sources = [("log", "jc", log)] if log and log.is_file() else []
        for i, d in enumerate(a.results_dir, 1):
            for f in sorted(Path(d).rglob("*")):
                if f.is_file() and f.stat().st_size < 64 << 20 and f.suffix in (".json", ".md", ".txt", ".log", ".jsonl", ""):
                    sources.append((f"results{i}.{f.relative_to(d)}", f"results {d}", f))
        scan_text_sources(res, index, sources)

    if person:
        person.delete()
    out = Path(a.out)
    meta = {"log": str(log) if log else "none", "results dirs": ", ".join(a.results_dir) or "none",
            "volume budget": f"{a.volume_budget:g} lines/min/module over {a.quiet_window:g} s without calls; "
                             f"p95 {a.lines_per_request} lines per request"}
    res.write(out, meta)
    # The suite's own output holds no secret either.
    if found:
        mine = ls.SecretIndex(found).find((out / "results.json").read_text())
        if mine:
            print(f"own results name {len(mine)} secret(s); results withheld", file=sys.stderr)
            (out / "results.json").write_text("{}")
            (out / "REPORT.md").write_text("withheld: the results named a secret value\n")
            return 1
    t = res.totals()
    print(json.dumps({"out": str(out), **t}))
    return 0 if t[FAIL] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
