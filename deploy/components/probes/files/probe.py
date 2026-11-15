#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Synthetic probes of every public surface, from outside the edge (T-5103).

One pod per cluster probes its own environment and the other one, through the public hosts, so
a cluster that is down entirely still goes red on the other side. Per target and minute it
spends at most PROBE_BUDGET requests, paced evenly over the minute:

- the catalogue search (CKAN package_search), the Portal's health and the public assistant's config;
- the Endpoints' records, each refreshed once per PROBE_DISCOVERY_SECONDS: the catalogue's
  `endpoint` extra lists the public Endpoints, the record's distributions name the surfaces
  each one serves, and `ngsi-ld/v1/types` the entity type every query below asks for;
- the rest split over the surface kinds: each kind probes its next Endpoints round robin, so a
  kind broken everywhere is red in the first minute, and one Endpoint's surface within
  ceil(endpoints / share) minutes.

Every check asserts content, not only the status: a count above zero, a non-empty JSON page, a
GeoJSON FeatureCollection with a Feature, a CSV header with `id`, a VEVENT, MCP tools, a
WebSocket 101 whose first message is `x-welcome`. Anonymous only: the probe holds no token and
sends none; a reason is the probe's own short sentence, never a response body.

Once an hour it also reads, from the same outside view, when each host's TLS certificate and each
registered domain expire (the domain from RDAP, its server from IANA's bootstrap list), for the
expiry alerts of T-5104.

Every five minutes it also reads, from inside, how old this cluster's newest Postgres backup and
restore drill are (T-5289), when PROBE_BACKUPS names the cluster: the CNPG Cluster's
lastSuccessfulBackup, the local dump CronJob's last success, and the drill's result ConfigMap. That
is the one call with a token: the pod's ServiceAccount, sent only to the in-cluster API server and
allowed `get` on those three objects and nothing else.

    GET /metrics       Prometheus text (jc_probe_*)
    GET /results.json  the newest result per target, Endpoint and surface (T-5102, T-5067)
    GET /healthz       200 while every target finished a round in the last three minutes

PROBE_TARGETS is a JSON list of {name, api, catalogue, portal, assistant, assistantId}; every URL is
https and every probe URL is built from them, never taken from an answer. The one exception is
the RDAP server, which comes from IANA's bootstrap answer: https only, no redirects, body capped.
"""

import base64
import json
import os
import re
import socket
import ssl
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

USER_AGENT = "jc-synthetic-probe/1 (T-5103)"
TIMEOUT = 10
BODY_CAP = 256 * 1024
NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,39}$")
SLUG = re.compile(r"^[a-z0-9]{1,64}$")
TYPE = re.compile(r"^[A-Za-z][A-Za-z0-9_]{0,99}$")
# Latency buckets of jc_probe_request_duration_seconds; 0.5 is the p95 objective of T-5104.
BUCKETS = (0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0)
EXPIRY_SECONDS = 3600
RDAP_BOOTSTRAP = "https://data.iana.org/rdap/dns.json"
BACKUP_SECONDS = 300
KUBE_API = "https://kubernetes.default.svc"
KUBE_SA = "/var/run/secrets/kubernetes.io/serviceaccount"
DRILL_RESULTS = {"passed", "failed", "skipped", "no-backup"}
# The surface paths of jc-dcat's Representation::Served (platform crates/jc-dcat/src/lib.rs).
SURFACES = {
    "ngsi-ld/v1/": "ngsi-ld",
    "odata/v4/": "odata",
    "ogc/features/": "ogc",
    "sta/v1.1/": "sta",
    "file.csv": "csv",
    "ical/v1/events.ics": "ical",
    "ws": "websocket",
    "mcp": "mcp",
}
TYPED = {"ngsi-ld", "odata", "ogc"}


# ---- checks: (status, headers with lower-case names, body bytes) -> None when it holds, else why


def _json(body):
    try:
        return json.loads(body)
    except ValueError:
        return None


def check_catalogue(status, headers, body):
    doc = _json(body)
    if status != 200:
        return f"status {status}"
    if not isinstance(doc, dict) or doc.get("success") is not True:
        return "no CKAN success answer"
    count = (doc.get("result") or {}).get("count")
    return None if isinstance(count, int) and count > 0 else "search found no dataset"


def check_assistant(status, headers, body):
    doc = _json(body)
    if status != 200:
        return f"status {status}"
    return None if isinstance(doc, dict) and doc.get("publicId") else "no assistant config"


def check_portal(status, headers, body):
    doc = _json(body)
    if status != 200:
        return f"status {status}"
    return None if isinstance(doc, dict) and doc.get("status") == "ok" else "the Portal reports no ok health"


def check_ngsi(status, headers, body):
    if status != 200:
        return f"status {status}"
    doc = _json(body)
    if not isinstance(doc, list) or not doc:
        return "no entity in the answer"
    count = headers.get("ngsild-results-count", "")
    return None if count.isdigit() and int(count) > 0 else "NGSILD-Results-Count missing or 0"


def check_odata(status, headers, body):
    if status != 200:
        return f"status {status}"
    doc = _json(body)
    rows = doc.get("value") if isinstance(doc, dict) else None
    return None if isinstance(rows, list) and rows else "no row in value"


def check_ogc(status, headers, body):
    if status != 200:
        return f"status {status}"
    doc = _json(body)
    if not isinstance(doc, dict) or doc.get("type") != "FeatureCollection":
        return "not a GeoJSON FeatureCollection"
    features = doc.get("features")
    if not isinstance(features, list) or not features:
        return "no feature in the collection"
    ok = all(isinstance(f, dict) and f.get("type") == "Feature" and "geometry" in f for f in features)
    return None if ok else "a feature is not GeoJSON"


def check_sta(status, headers, body):
    if status != 200:
        return f"status {status}"
    doc = _json(body)
    rows = doc.get("value") if isinstance(doc, dict) else None
    if not isinstance(rows, list) or not rows:
        return "no Thing in value"
    return None if all(isinstance(r, dict) and "@iot.id" in r for r in rows) else "a Thing has no @iot.id"


def check_csv(status, headers, body):
    if status != 200:
        return f"status {status}"
    if not headers.get("content-type", "").startswith("text/csv"):
        return "not text/csv"
    lines = body.decode("utf-8", "replace").splitlines()
    if not lines or "id" not in lines[0].split(","):
        return "no id column in the header"
    return None if len(lines) > 1 else "no row after the header"


def check_ical(status, headers, body):
    if status != 200:
        return f"status {status}"
    text = body.decode("utf-8", "replace")
    if not text.startswith("BEGIN:VCALENDAR"):
        return "not an iCalendar"
    return None if "BEGIN:VEVENT" in text else "no VEVENT"


def check_mcp(status, headers, body):
    if status != 200:
        return f"status {status}"
    if headers.get("content-type", "").startswith("text/event-stream"):
        # Streamable HTTP may answer as one server-sent event: its data line is the JSON-RPC answer.
        body = next((line[5:] for line in body.decode("utf-8", "replace").splitlines() if line.startswith("data:")), "")
    doc = _json(body)
    tools = ((doc or {}).get("result") or {}).get("tools") if isinstance(doc, dict) else None
    if not isinstance(tools, list) or not tools:
        return "tools/list returned no tool"
    return None if all(isinstance(t, dict) and t.get("name") for t in tools) else "a tool has no name"


def check_websocket(status, headers, body):
    if status != 101:
        return f"status {status}"
    if headers.get("sec-websocket-protocol") != "ngsi-ld-ws":
        return "subprotocol ngsi-ld-ws not chosen"
    doc = _json(body)
    op = ((doc or {}).get("metadata") or {}).get("x-op") if isinstance(doc, dict) else None
    return None if op == "x-welcome" else "first message is not x-welcome"


CHECKS = {
    "catalogue": check_catalogue,
    "assistant": check_assistant,
    "portal": check_portal,
    "ngsi-ld": check_ngsi,
    "odata": check_odata,
    "ogc": check_ogc,
    "sta": check_sta,
    "csv": check_csv,
    "ical": check_ical,
    "mcp": check_mcp,
    "websocket": check_websocket,
}


# ---- transport


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is the answer judged, never followed to a host the probe was not given."""

    def redirect_request(self, *args, **kwargs):
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)


def fetch(url, data=None, headers=None, cap=BODY_CAP):
    """(status, headers, at most `cap` bytes of the body, seconds); status 0 when nothing answered."""
    request = urllib.request.Request(  # noqa: S310 (https origins, checked at load)
        url, data=data, headers={"User-Agent": USER_AGENT, **(headers or {})})
    start = time.monotonic()
    try:
        with _OPENER.open(request, timeout=TIMEOUT) as answer:  # noqa: S310 (https origins, checked at load)
            return answer.status, {k.lower(): v for k, v in answer.headers.items()}, answer.read(cap), time.monotonic() - start
    except urllib.error.HTTPError as err:
        return err.code, {k.lower(): v for k, v in err.headers.items()}, err.read(cap), time.monotonic() - start
    except (OSError, ValueError) as err:
        return 0, {}, str(getattr(err, "reason", err)).encode()[:200], time.monotonic() - start


def _exactly(sock, n):
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ConnectionError("closed")
        data += chunk
    return data


def websocket(url):
    """Open the Endpoint's WebSocket anonymously: (status, headers, first text message, seconds)."""
    parts = urllib.parse.urlsplit(url)
    start = time.monotonic()
    key = base64.b64encode(os.urandom(16)).decode()
    try:
        raw = socket.create_connection((parts.hostname, parts.port or 443), timeout=TIMEOUT)
        with ssl.create_default_context().wrap_socket(raw, server_hostname=parts.hostname) as sock:
            sock.sendall(
                f"GET {parts.path} HTTP/1.1\r\nHost: {parts.hostname}\r\nUpgrade: websocket\r\n"
                f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
                f"Sec-WebSocket-Protocol: ngsi-ld-ws\r\nUser-Agent: {USER_AGENT}\r\n\r\n".encode()
            )
            head = b""
            while b"\r\n\r\n" not in head and len(head) < 16384:
                chunk = sock.recv(4096)
                if not chunk:
                    break
                head += chunk
            lines = head.split(b"\r\n\r\n", 1)[0].decode("latin-1").split("\r\n")
            status = int(lines[0].split(" ")[1]) if len(lines[0].split(" ")) > 1 else 0
            headers = {}
            for line in lines[1:]:
                name, _, value = line.partition(":")
                headers[name.strip().lower()] = value.strip()
            message = b""
            if status == 101:
                rest = head.split(b"\r\n\r\n", 1)[1]
                b0, b1 = (rest + _exactly(sock, max(0, 2 - len(rest))))[:2]
                rest = rest[2:]
                size = b1 & 0x7F
                extra = {126: 2, 127: 8}.get(size, 0)
                if extra:
                    rest += _exactly(sock, max(0, extra - len(rest)))
                    size, rest = int.from_bytes(rest[:extra], "big"), rest[extra:]
                if size > BODY_CAP:
                    return status, headers, b"", time.monotonic() - start
                message = (rest + _exactly(sock, max(0, size - len(rest))))[:size]
                # A masked close frame with no payload, as RFC 6455 5.3 asks of a client.
                sock.sendall(bytes([0x88, 0x80]) + os.urandom(4))
            return status, headers, message, time.monotonic() - start
    except (OSError, ValueError, ConnectionError) as err:
        return 0, {}, str(err).encode()[:200], time.monotonic() - start


# ---- targets, discovery and the round


def tls_not_after(host):
    """When the certificate `host` serves on 443 expires; 0 when it fails verification (expired or
    wrong), None when nothing answered, so a network blip never reads as an expired certificate."""
    try:
        with socket.create_connection((host, 443), timeout=TIMEOUT) as raw, \
                ssl.create_default_context().wrap_socket(raw, server_hostname=host) as tls:
            return ssl.cert_time_to_seconds(tls.getpeercert()["notAfter"])
    except ssl.SSLCertVerificationError:
        return 0.0
    except (OSError, KeyError, ValueError):
        return None


def registered_domain(host):
    # ponytail: the last two labels; a two-label suffix (.co.uk) needs the public suffix list.
    return ".".join(host.split(".")[-2:])


def domain_not_after(domain, get=fetch):
    """The registration's RDAP `expiration` event as a timestamp, None when no answer holds one."""
    status, _, body, _ = get(RDAP_BOOTSTRAP, None, {"Accept": "application/json"}, 1024 * 1024)
    services = (_json(body) or {}).get("services") if status == 200 and isinstance(_json(body), dict) else None
    tld = domain.rsplit(".", 1)[-1]
    base = next((urls[0] for tlds, urls in services or [] if tld in tlds and urls and str(urls[0]).startswith("https://")), None)
    if base is None:
        return None
    status, _, body, _ = get(f"{base.rstrip('/')}/domain/{domain}", None, {"Accept": "application/rdap+json"})
    doc = _json(body) if status == 200 else None
    for event in (doc or {}).get("events") or [] if isinstance(doc, dict) else []:
        if isinstance(event, dict) and event.get("eventAction") == "expiration":
            try:
                return datetime.fromisoformat(str(event.get("eventDate"))).timestamp()
            except ValueError:
                return None
    return None


def kube_get(path):
    """One GET of the in-cluster API server with the pod's own token; the parsed JSON or None."""
    try:
        with open(f"{KUBE_SA}/token", encoding="ascii") as f:
            token = f.read().strip()
        context = ssl.create_default_context(cafile=f"{KUBE_SA}/ca.crt")
        request = urllib.request.Request(  # noqa: S310 (a fixed in-cluster https URL)
            KUBE_API + path, headers={"Authorization": f"Bearer {token}", "Accept": "application/json"})
        with urllib.request.urlopen(request, timeout=TIMEOUT, context=context) as answer:  # noqa: S310
            return json.loads(answer.read(BODY_CAP))
    except (OSError, ValueError):
        return None


def _when(value):
    """An RFC 3339 instant from the API as a timestamp; None for anything else."""
    try:
        return datetime.fromisoformat(str(value)).timestamp() if value else None
    except ValueError:
        return None


def load_backups(text):
    """PROBE_BACKUPS: {namespace, cluster, localBackup} or empty for a cluster without Postgres."""
    if not text:
        return None
    cfg = json.loads(text)
    for key in ("namespace", "cluster"):
        if not isinstance(cfg.get(key), str) or not NAME.match(cfg[key]):
            raise ValueError(f"PROBE_BACKUPS {key} must match {NAME.pattern}")
    return {"namespace": cfg["namespace"], "cluster": cfg["cluster"], "localBackup": cfg.get("localBackup") is True}


def backup_state(cfg, get=kube_get):
    """The newest backup and drill of the cluster, read from the API: {backupAt, drillAt, drillResult}.

    The newest backup is the later of CNPG's last successful base backup and the local dump's last
    successful run; a value the API does not give stays None, so the page shows it as unknown."""
    ns, name = cfg["namespace"], cfg["cluster"]
    cluster = get(f"/apis/postgresql.cnpg.io/v1/namespaces/{ns}/clusters/{name}") or {}
    found = [_when((cluster.get("status") or {}).get("lastSuccessfulBackup"))]
    if cfg["localBackup"]:
        cron = get(f"/apis/batch/v1/namespaces/{ns}/cronjobs/{name}-local-backup") or {}
        found.append(_when((cron.get("status") or {}).get("lastSuccessfulTime")))
    drill = (get(f"/api/v1/namespaces/{ns}/configmaps/{name}-dr-drill") or {}).get("data") or {}
    result = drill.get("result")
    return {
        "backupAt": max((t for t in found if t is not None), default=None),
        "drillAt": _when(drill.get("at")),
        "drillResult": result if result in DRILL_RESULTS else None,
    }


def load_targets(text):
    """PROBE_TARGETS, validated: a bad entry stops the pod at start instead of probing elsewhere."""
    targets = json.loads(text)
    if not isinstance(targets, list) or not targets:
        raise ValueError("PROBE_TARGETS must be a non-empty JSON list")
    for t in targets:
        if not isinstance(t, dict) or not NAME.match(str(t.get("name", ""))):
            raise ValueError(f"target name must match {NAME.pattern}: {t!r}")
        for key in ("api", "catalogue", "portal", "assistant"):
            url = urllib.parse.urlsplit(str(t.get(key, "")))
            if url.scheme != "https" or not url.hostname or url.path not in ("", "/") or url.query:
                raise ValueError(f"target {t['name']}: {key} must be an https origin")
            t[key] = f"https://{url.netloc}"
        if not NAME.match(str(t.get("assistantId", ""))):
            raise ValueError(f"target {t['name']}: assistantId must match {NAME.pattern}")
    return targets


def endpoint_slugs(api, doc):
    """Slugs of the catalogue's `endpoint` extras that name an Endpoint on this target's own api."""
    prefix = f"{api}/api/endpoint/"
    slugs = []
    for package in ((doc or {}).get("result") or {}).get("results") or []:
        url = str(package.get("endpoint") or "") if isinstance(package, dict) else ""
        slug = url[len(prefix):].strip("/") if url.startswith(prefix) else ""
        if SLUG.match(slug) and slug not in slugs:
            slugs.append(slug)
    return slugs


def record_surfaces(api, slug, doc):
    """The surfaces an Endpoint record lists in its distributions, by kind."""
    base = f"{api}/api/endpoint/{slug}/"
    found = set()
    distributions = doc.get("dcat:distribution") if isinstance(doc, dict) else None
    for dist in distributions if isinstance(distributions, list) else []:
        url = str(dist.get("dcat:accessURL") or "") if isinstance(dist, dict) else ""
        if url.startswith(base) and url[len(base):] in SURFACES:
            found.add(SURFACES[url[len(base):]])
    return sorted(found)


def surface_request(api, slug, kind, entity_type):
    """(url, POST body or None, headers, body cap) of one surface's probe."""
    base = f"{api}/api/endpoint/{slug}"
    t = urllib.parse.quote(entity_type or "", safe="")
    return {
        "ngsi-ld": (f"{base}/ngsi-ld/v1/entities?type={t}&limit=1&count=true", None, {"Accept": "application/json"}, BODY_CAP),
        "odata": (f"{base}/odata/v4/{t}?$top=1", None, {"Accept": "application/json"}, BODY_CAP),
        "ogc": (f"{base}/ogc/features/collections/{t}/items?limit=1", None, {"Accept": "application/geo+json"}, BODY_CAP),
        "sta": (f"{base}/sta/v1.1/Things?$top=1", None, {"Accept": "application/json"}, BODY_CAP),
        "csv": (f"{base}/file.csv", None, {}, 64 * 1024),
        "ical": (f"{base}/ical/v1/events.ics", None, {}, BODY_CAP),
        "mcp": (
            f"{base}/mcp",
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).encode(),
            {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
            BODY_CAP,
        ),
        "websocket": (f"{base}/ws", None, {}, BODY_CAP),
    }[kind]


def plan(budget, kinds, cursors, holders):
    """Which Endpoints each surface kind probes this round: `budget` split over the kinds, each
    taking its next Endpoints round robin; `cursors` carries the position to the next round.
    Every kind gets one probe even when the budget is smaller than the kinds (T-5184): a kind
    left out for a round would go stale on the status page."""
    if not kinds:
        return []
    share = max(1, budget // len(kinds))
    picks = []
    for kind in kinds:
        slugs = holders[kind]
        take = min(share, len(slugs))
        start = cursors.get(kind, 0) % len(slugs)
        picks += [(kind, slugs[(start + i) % len(slugs)]) for i in range(take)]
        cursors[kind] = start + take
    return picks


class Prober:
    def __init__(self, targets, budget, discovery_seconds, clock=time.time, sleep=time.sleep, get=fetch, ws=websocket,
                 tls=tls_not_after, domain=domain_not_after, backups=None, kube=kube_get):
        self.targets, self.budget, self.discovery_seconds = targets, budget, discovery_seconds
        self.clock, self.sleep, self.get, self.ws, self.tls, self.domain = clock, sleep, get, ws, tls, domain
        self.lock = threading.Lock()
        self.results = {}  # (target, endpoint, surface) -> {ok, reason, seconds, at}
        self.counts = {}  # (target, surface) -> {failures, buckets, sum, count}: the SLO's events
        self.expiry = {}  # ("cert", host) or ("domain", domain) -> expiry timestamp
        self.rounds = {}  # target -> time of the last finished round
        self.endpoints = {t["name"]: {} for t in targets}  # target -> slug -> {surfaces, type, at}
        self.listed_at = {}
        self.cursors = {t["name"]: {} for t in targets}
        self.started = clock()
        self.backups_cfg, self.kube = backups, kube
        self.backups = None  # backup_state() of this cluster, once read

    def record(self, target, endpoint, surface, reason, seconds):
        with self.lock:
            self.results[(target, endpoint, surface)] = {
                "ok": reason is None, "reason": reason, "seconds": round(seconds, 3), "at": self.clock(),
            }
            c = self.counts.setdefault((target, surface), {"failures": 0, "buckets": [0] * len(BUCKETS), "sum": 0.0, "count": 0})
            c["failures"] += 0 if reason is None else 1
            c["count"] += 1
            c["sum"] += seconds
            for i, bound in enumerate(BUCKETS):
                c["buckets"][i] += 1 if seconds <= bound else 0

    def expiries(self, target):
        """Certificate expiry of the target's hosts and its registered domain; a None keeps the last."""
        hosts = sorted({urllib.parse.urlsplit(target[k]).hostname for k in ("api", "catalogue", "assistant")})
        found = {("cert", h): self.tls(h) for h in hosts}
        for name in sorted({registered_domain(h) for h in hosts}):
            found[("domain", name)] = self.domain(name, self.get)
        with self.lock:
            self.expiry.update({k: v for k, v in found.items() if v is not None})

    def _paced(self, call, *args):
        start = time.monotonic()
        answer = call(*args)
        self.sleep(max(0.0, 60.0 / self.budget - (time.monotonic() - start)))
        return answer

    def round(self, target):
        name, api, spent = target["name"], target["api"], 0
        status, headers, body, secs = self._paced(self.get, f"{target['catalogue']}/api/3/action/package_search?rows=1")
        self.record(name, "", "catalogue", check_catalogue(status, headers, body), secs)
        status, headers, body, secs = self._paced(
            self.get, f"{target['assistant']}/api/v1/d/{target['assistantId']}/config", None, {"Accept": "application/json"})
        self.record(name, "", "assistant", check_assistant(status, headers, body), secs)
        status, headers, body, secs = self._paced(
            self.get, f"{target['portal']}/api/v1/health", None, {"Accept": "application/json"})
        self.record(name, "", "portal", check_portal(status, headers, body), secs)
        spent += 3
        known = self.endpoints[name]
        # ponytail: one page of 1000 datasets; page with start= once a catalogue holds more.
        if self.clock() - self.listed_at.get(name, 0) >= self.discovery_seconds:
            status, _, body, _ = self._paced(
                self.get, f"{target['catalogue']}/api/3/action/package_search?rows=1000&fl=extras_endpoint",
                None, {}, 4 * 1024 * 1024)
            spent += 1
            if status == 200 and _json(body) is not None:
                listed = endpoint_slugs(api, _json(body))
                for gone in set(known) - set(listed):
                    del known[gone]
                with self.lock:
                    for key in [k for k in self.results if k[0] == name and k[1] and k[1] not in listed]:
                        del self.results[key]
                for slug in listed:
                    known.setdefault(slug, {"surfaces": [], "type": None, "at": 0})
                self.listed_at[name] = self.clock()
        # Up to a quarter of the budget refreshes the oldest records, at least one a round; the rest probes.
        refreshed = 0
        for slug in sorted(known, key=lambda s: known[s]["at"]):
            if self.clock() - known[slug]["at"] < self.discovery_seconds or (refreshed and spent + 2 > self.budget // 4):
                break
            status, _, body, _ = self._paced(self.get, f"{api}/api/endpoint/{slug}", None, {"Accept": "application/ld+json"})
            spent, refreshed = spent + 1, refreshed + 1
            if status != 200:
                continue
            surfaces = record_surfaces(api, slug, _json(body))
            entity_type = None
            if TYPED & set(surfaces):
                status, _, body, _ = self._paced(
                    self.get, f"{api}/api/endpoint/{slug}/ngsi-ld/v1/types", None, {"Accept": "application/json"})
                spent += 1
                doc = _json(body)
                if status != 200 or not isinstance(doc, dict):
                    # A failed read is no answer about the Endpoint (T-5660): keep the type the last good one
                    # learned (None: not probed yet) and the old `at`, so the next round asks again.
                    print(json.dumps({"target": name, "endpoint": slug, "discovery": f"status {status}"}), file=sys.stderr, flush=True)
                    known[slug] = {**known[slug], "surfaces": surfaces}
                    continue
                types = doc.get("typeList")
                # "" (not None): the Endpoint answered and lists no usable type, a failure of its typed surfaces.
                entity_type = next((t for t in types if isinstance(t, str) and TYPE.match(t)), "") if isinstance(types, list) else ""
            known[slug] = {"surfaces": surfaces, "type": entity_type, "at": self.clock()}
        holders = {}
        for slug in sorted(known):
            for kind in known[slug]["surfaces"]:
                if kind in TYPED and known[slug]["type"] is None:
                    continue  # no type learned yet: neither up nor down until a discovery answers
                holders.setdefault(kind, []).append(slug)
        for kind, slug in plan(self.budget - spent, sorted(holders), self.cursors[name], holders):
            entity_type = known[slug]["type"]
            if kind in TYPED and not entity_type:
                self.record(name, slug, kind, "the Endpoint lists no entity type", 0.0)
                continue
            url, data, headers, cap = surface_request(api, slug, kind, entity_type)
            if kind == "websocket":
                status, headers, body, secs = self._paced(self.ws, url)
            else:
                status, headers, body, secs = self._paced(self.get, url, data, headers, cap)
            self.record(name, slug, kind, CHECKS[kind](status, headers, body), secs)
        with self.lock:
            self.rounds[name] = self.clock()

    def run(self, target):
        checked = 0.0
        while True:
            try:
                if self.clock() - checked >= EXPIRY_SECONDS:
                    checked = self.clock()
                    self.expiries(target)
                self.round(target)
            except Exception as err:  # noqa: BLE001 (one bad answer must not stop the probe)
                print(json.dumps({"target": target["name"], "error": type(err).__name__}), file=sys.stderr, flush=True)
                self.sleep(10)

    def watch_backups(self):
        while True:
            try:
                state = backup_state(self.backups_cfg, self.kube)
                with self.lock:
                    self.backups = state
            except Exception as err:  # noqa: BLE001 (one bad answer must not stop the probe)
                print(json.dumps({"backups": "error", "error": type(err).__name__}), file=sys.stderr, flush=True)
            self.sleep(BACKUP_SECONDS)

    def metrics(self):
        def esc(value):
            return str(value).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ")

        with self.lock:
            items = sorted(self.results.items())
            rounds = dict(self.rounds)
            counts = {k: {**v, "buckets": list(v["buckets"])} for k, v in sorted(self.counts.items())}
            expiry = sorted(self.expiry.items())
            backups = dict(self.backups or {})
        out = [
            "# HELP jc_probe_up 1 when the newest probe of the surface held, 0 when it did not.",
            "# TYPE jc_probe_up gauge",
        ]
        labels = []
        for (target, endpoint, surface), r in items:
            label = f'target="{esc(target)}",endpoint="{esc(endpoint)}",surface="{esc(surface)}"'
            labels.append((label, r))
            out.append(f"jc_probe_up{{{label}}} {1 if r['ok'] else 0}")
        out += ["# HELP jc_probe_duration_seconds Seconds the newest probe took.", "# TYPE jc_probe_duration_seconds gauge"]
        out += [f"jc_probe_duration_seconds{{{label}}} {r['seconds']}" for label, r in labels]
        out += ["# HELP jc_probe_timestamp_seconds When the newest probe ran.", "# TYPE jc_probe_timestamp_seconds gauge"]
        out += [f"jc_probe_timestamp_seconds{{{label}}} {int(r['at'])}" for label, r in labels]
        failing = {}
        for (target, _, surface), r in items:
            failing.setdefault((target, surface), 0)
            failing[(target, surface)] += 0 if r["ok"] else 1
        out += ["# HELP jc_probe_surface_failing Endpoints whose newest probe of the surface failed.",
                "# TYPE jc_probe_surface_failing gauge"]
        out += [f'jc_probe_surface_failing{{target="{esc(t)}",surface="{esc(s)}"}} {n}' for (t, s), n in sorted(failing.items())]
        out += ["# HELP jc_probe_round_timestamp_seconds When the target's newest round finished.",
                "# TYPE jc_probe_round_timestamp_seconds gauge"]
        out += [f'jc_probe_round_timestamp_seconds{{target="{esc(t)}"}} {int(at)}' for t, at in sorted(rounds.items())]
        out += ["# HELP jc_probe_failures_total Probes of the surface that failed their check.",
                "# TYPE jc_probe_failures_total counter"]
        out += [f'jc_probe_failures_total{{target="{esc(t)}",surface="{esc(s)}"}} {c["failures"]}' for (t, s), c in counts.items()]
        out += ["# HELP jc_probe_request_duration_seconds Seconds each probe of the surface took.",
                "# TYPE jc_probe_request_duration_seconds histogram"]
        for (t, s), c in counts.items():
            label = f'target="{esc(t)}",surface="{esc(s)}"'
            out += [f'jc_probe_request_duration_seconds_bucket{{{label},le="{b}"}} {n}' for b, n in zip(BUCKETS, c["buckets"])]
            out += [f'jc_probe_request_duration_seconds_bucket{{{label},le="+Inf"}} {c["count"]}',
                    f"jc_probe_request_duration_seconds_sum{{{label}}} {round(c['sum'], 3)}",
                    f"jc_probe_request_duration_seconds_count{{{label}}} {c['count']}"]
        out += ["# HELP jc_probe_tls_cert_expiry_timestamp_seconds When the host's certificate expires (0: it fails verification).",
                "# TYPE jc_probe_tls_cert_expiry_timestamp_seconds gauge"]
        out += [f'jc_probe_tls_cert_expiry_timestamp_seconds{{host="{esc(n)}"}} {int(v)}' for (k, n), v in expiry if k == "cert"]
        out += ["# HELP jc_probe_domain_expiry_timestamp_seconds When the domain's registration expires (RDAP).",
                "# TYPE jc_probe_domain_expiry_timestamp_seconds gauge"]
        out += [f'jc_probe_domain_expiry_timestamp_seconds{{domain="{esc(n)}"}} {int(v)}' for (k, n), v in expiry if k == "domain"]
        if backups.get("backupAt") is not None:
            out += ["# HELP jc_probe_backup_timestamp_seconds When this cluster's newest Postgres backup finished.",
                    "# TYPE jc_probe_backup_timestamp_seconds gauge", f"jc_probe_backup_timestamp_seconds {int(backups['backupAt'])}"]
        if backups.get("drillAt") is not None:
            out += ["# HELP jc_probe_restore_drill_timestamp_seconds When the newest restore drill finished.",
                    "# TYPE jc_probe_restore_drill_timestamp_seconds gauge",
                    f"jc_probe_restore_drill_timestamp_seconds {int(backups['drillAt'])}",
                    "# HELP jc_probe_restore_drill_passed 1 when the newest restore drill passed.",
                    "# TYPE jc_probe_restore_drill_passed gauge",
                    f"jc_probe_restore_drill_passed {1 if backups.get('drillResult') == 'passed' else 0}"]
        return "\n".join(out) + "\n"

    def snapshot(self):
        with self.lock:
            return [
                {"target": t, "endpoint": e or None, "surface": s, **r}
                for (t, e, s), r in sorted(self.results.items())
            ]

    def backup_snapshot(self):
        with self.lock:
            return None if self.backups is None else dict(self.backups)

    def healthy(self):
        with self.lock:
            return all(self.clock() - self.rounds.get(t["name"], self.started) < 180 for t in self.targets)


def serve(prober, port):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 (http.server's name)
            if self.path == "/metrics":
                body, kind, status = prober.metrics().encode(), "text/plain; version=0.0.4", 200
            elif self.path == "/results.json":
                doc = {"results": prober.snapshot(), "backups": prober.backup_snapshot()}
                body, kind, status = json.dumps(doc).encode(), "application/json", 200
            elif self.path == "/healthz":
                ok = prober.healthy()
                body, kind, status = (b"ok" if ok else b"a target missed its round"), "text/plain", 200 if ok else 503
            else:
                body, kind, status = b"not found", "text/plain", 404
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()  # noqa: S104 (the pod's own port)


def main():
    budget = int(os.environ.get("PROBE_BUDGET", "120"))
    # Under the edge's 300 requests a minute per address with room for the other suites.
    if not 12 <= budget <= 200:
        raise ValueError("PROBE_BUDGET must be 12..200 requests a minute per target")
    prober = Prober(load_targets(os.environ["PROBE_TARGETS"]), budget, int(os.environ.get("PROBE_DISCOVERY_SECONDS", "900")),
                    backups=load_backups(os.environ.get("PROBE_BACKUPS", "")))
    for target in prober.targets:
        threading.Thread(target=prober.run, args=(target,), daemon=True).start()
    if prober.backups_cfg:
        threading.Thread(target=prober.watch_backups, daemon=True).start()
    serve(prober, int(os.environ.get("PROBE_PORT", "9100")))


if __name__ == "__main__":
    main()
