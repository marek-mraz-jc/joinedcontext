"""Live mode: the anonymous read parts of the smoke suites against one named jc deployment on a
cluster (jc-prod, jc-dev), through its APISIX edge, before or after DNS points at it.

`run all --deployment <name> --domain <d> --edge <ip> --jc-config <jc.toml>` puts the suites on a
virtual base `https://<name>.smoke.invalid` (a name that never resolves) and installs one guard in
its own process and in every suite process (`run` installs it from $JC_SMOKE_LIVE):

- Routing: a path on the virtual base goes to the host jc's host-mode mounts serve it on, as the
  rehearsal's slug mounts did (`/id/...` -> idm.<d>/..., `/opendata/...` -> opendata.<d>/...,
  the gateway's root paths -> its first host, anything else -> the Portal's host). Each request
  connects to the edge IP and names the real host in Host and SNI; TLS is verified.
- Hosts: only the domain's hosts the jc.toml mounts; any other host, a raw socket, an unverified
  TLS context and every subprocess but another `run` are refused.
- Requests: GET, HEAD and OPTIONS; POST only to the anonymous MCP JSON-RPC paths and to the
  anonymous write refusal probe; never an Authorization or Cookie header. A refused request
  raises ConnectionRefusedError naming it (the suite's row fails with that text) and is logged.
- Rate: one cap shared by every thread and process (a lock file), default 5 requests/s, and
  anonymous MCP calls under the gateway's per-address limit (MCP_RATE).
"""

from __future__ import annotations

import fcntl
import http.client
import ipaddress
import json
import os
import re
import socket
import ssl
import subprocess
import sys
import threading
import time
import tomllib
import urllib.parse
from pathlib import Path

ENV = "JC_SMOKE_LIVE"
SMOKE = Path(__file__).resolve().parent

# bins/jc/src/module.rs Module::slug: where the rehearsal's slug mounts put each module.
SLUGS = {"kaisatsu": "id", "dataroom": "opendata", "daiya": "dashboards", "viz": "viz",
         "assistant": "assistant", "model-tools": "model-tools"}
# Module::root_paths of the gateway: it answers at these whole paths on every host.
GATEWAY_ROOTS = ("/api/endpoint", "/api/endpoints", "/api/mcp", "/cs", "/catalog", "/catalog.jsonld",
                 "/catalog.ttl", "/oauth", "/.well-known/oauth-authorization-server",
                 "/.well-known/oauth-protected-resource/api/mcp",
                 "/.well-known/oauth-protected-resource/api/endpoint")
READ_METHODS = {"GET", "HEAD", "OPTIONS"}
# POST, anonymous only: the MCP JSON-RPC doors and the gateway's anonymous write refusal probe.
MCP_PATHS = (re.compile(r"/api/mcp(/[^/?]+/[^/?]+)?/?"), re.compile(r"/api/endpoint/[^/?]+/mcp/?"))
POST_PATHS = (*MCP_PATHS, re.compile(r"/api/endpoint/[a-z2-7]+/ngsi-ld/v1/entities"))
# Behind the edge the gateway counts anonymous MCP per client address: 120 a minute, burst 30
# (context-gateway ANONYMOUS_MCP_RATE_LIMIT, spend_anonymous_mcp). A loopback rehearsal carries no
# X-Forwarded-For and is never counted, so the suites' own pacing is faster than that.
MCP_RATE = 1.8
SECRET_HEADERS = {"authorization", "cookie", "proxy-authorization"}
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,62}")


class Refused(ConnectionRefusedError):
    pass


def under(host: str, domain: str) -> bool:
    return "*" not in host and (host == domain or host.endswith("." + domain))


def routes(jc_toml: str, domain: str) -> tuple[list[tuple[str, str, str]], set[str]]:
    """([(virtual prefix, host, real prefix)] longest first, every host) of a host-mode jc.toml.
    Entries on a host outside `domain` (localhost:8080, a wildcard) are left out: never reached."""
    doc = tomllib.loads(jc_toml)
    if doc.get("mode") != "host":
        raise SystemExit("--jc-config: not a host-mode jc.toml (mode = \"host\")")
    if doc.get("base_host", "").lower() != domain:
        raise SystemExit(f"--jc-config: base_host {doc.get('base_host')!r} is not --domain {domain!r}")
    out, hosts = [], set()
    for module, entries in (doc.get("mounts") or {}).items():
        entries = entries if isinstance(entries, list) else [entries]
        mine = [(e["host"].lower(), e.get("path_prefix", "") or "") for e in entries
                if under(e.get("host", "").lower(), domain)]
        if not mine:
            continue
        hosts |= {h for h, _ in mine}
        host, prefix = mine[0]
        if module == "gateway":
            out += [(r, host, r) for r in GATEWAY_ROOTS]
        elif module == "portal":
            out.append(("", host, prefix))
        elif module in SLUGS:
            out.append(("/" + SLUGS[module], host, prefix))
    if not any(v == "" for v, _, _ in out):
        raise SystemExit("--jc-config mounts no Portal on the domain: nowhere to send the other paths")
    return sorted(out, key=lambda r: -len(r[0])), hosts


def config(deployment: str, domain: str, edge: str, jc_toml: str, out: Path, rate: float = 5.0,
           edge_port: int = 443, scheme: str = "https") -> dict:
    domain = domain.lower().rstrip(".")
    if not NAME.fullmatch(deployment):
        raise SystemExit(f"--deployment {deployment!r}: a lower-case name")
    if not re.fullmatch(r"[a-z0-9.-]+\.[a-z]{2,}", domain):
        raise SystemExit(f"--domain {domain!r}: a plain domain name")
    ipaddress.ip_address(edge)
    if rate <= 0:
        raise SystemExit("--rate: above 0")
    table, hosts = routes(jc_toml, domain)
    out.mkdir(parents=True, exist_ok=True)
    return {"deployment": deployment, "domain": domain, "edge": edge, "edge_port": edge_port,
            "scheme": scheme, "virtual": f"{deployment}.smoke.invalid", "routes": table,
            "hosts": sorted(hosts), "rate": rate, "bucket": str(out / ".rate"),
            "log": str(out / "refused.jsonl")}


def base_url(cfg: dict) -> str:
    return f"{cfg['scheme']}://{cfg['virtual']}"


# ------------------------------------------------------------------------------------ guard

class Guard:
    def __init__(self, cfg: dict):
        self.cfg = cfg
        self.hosts = set(cfg["hosts"])
        self.local = threading.local()
        self.lock = threading.Lock()

    def route(self, host: str, url: str) -> tuple[str, str]:
        """(real host, real path) of a request on `host`, or Refused."""
        u = urllib.parse.urlsplit(url)
        if u.netloc:  # an absolute request target names its own host
            host = u.hostname or ""
        path = u.path or "/"
        query = f"?{u.query}" if u.query else ""
        host = host.lower()
        if host == self.cfg["virtual"]:
            for virtual, real, prefix in self.cfg["routes"]:
                if virtual == "" or path == virtual or path.startswith(virtual + "/"):
                    # The gateway's entries name its root paths (virtual == prefix): kept whole.
                    return real, (prefix + path[len(virtual):] or "/") + query
        if host in self.hosts:
            return host, path + query
        raise Refused(f"host {host or '?'} is not a host of {self.cfg['deployment']}")

    def method(self, method: str, path: str) -> None:
        if method in READ_METHODS:
            return
        bare = path.split("?", 1)[0]
        if method == "POST" and any(p.fullmatch(bare) for p in POST_PATHS):
            return
        raise Refused(f"{method} {bare}: write row, rehearsal only")

    def refuse(self, what: str, why: str):
        line = json.dumps({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "request": what,
                           "why": why, "pid": os.getpid()})
        with self.lock, open(self.cfg["log"], "a") as f:
            f.write(line + "\n")
        raise Refused(f"jc-smoke live guard refused {what}: {why}")

    def wait(self, bucket: str | None = None, rate: float | None = None) -> None:
        """One slot of the shared rate: the bucket file holds the time the next request may go."""
        with open(bucket or self.cfg["bucket"], "a+") as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            f.seek(0)
            try:
                nxt = float(f.read() or 0)
            except ValueError:
                nxt = 0.0
            now = time.time()
            slot = max(now, nxt)
            f.seek(0)
            f.truncate()
            f.write(repr(slot + 1 / (rate or self.cfg["rate"])))
        if slot > now:
            time.sleep(slot - now)


ACTIVE: Guard | None = None
_SAVED: dict = {}


def active() -> Guard | None:
    return ACTIVE


def serves(base: str) -> bool:
    """Whether `base` is the virtual base of the guard installed in this process."""
    host = (urllib.parse.urlparse(base).hostname or "").lower()
    return ACTIVE is not None and host == ACTIVE.cfg["virtual"]


def install(cfg: dict) -> Guard:
    global ACTIVE
    if ACTIVE is not None:
        return ACTIVE
    g = Guard(cfg)
    HC, HS = http.client.HTTPConnection, http.client.HTTPSConnection
    _SAVED.update(putrequest=HC.putrequest, putheader=HC.putheader, connect=HC.connect, sconnect=HS.connect,
                  create_connection=socket.create_connection, sock_connect=socket.socket.connect,
                  sock_connect_ex=socket.socket.connect_ex, popen=subprocess.Popen.__init__)

    def putrequest(self, method, url, *a, **k):
        what = f"{method} {self.host}{url if url.startswith('/') else ' ' + url}"
        scheme = "https" if isinstance(self, HS) else "http"
        if scheme != cfg["scheme"]:
            g.refuse(what, f"{scheme}: the live mode speaks {cfg['scheme']} only")
        try:
            host, path = g.route(self.host, url)
            g.method(method, path)
        except Refused as e:
            g.refuse(what, str(e))
        if self.sock is not None and host != self.host:
            g.refuse(what, "a connection open to another host")
        self.host = host
        self._jc_live = f"{method} {host}{path}"
        if method == "POST" and any(p.fullmatch(path.split("?", 1)[0]) for p in MCP_PATHS):
            g.wait(cfg["bucket"] + "-mcp", min(MCP_RATE, cfg["rate"]))
        g.wait()
        return _SAVED["putrequest"](self, method, path, *a, **k)

    def putheader(self, header, *values):
        name = header.decode() if isinstance(header, bytes) else str(header)
        if name.lower() in SECRET_HEADERS:
            g.refuse(getattr(self, "_jc_live", self.host), f"{name} header: the live mode is anonymous")
        if name.lower() == "host":
            values = (self.host,)
        return _SAVED["putheader"](self, header, *values)

    def connect(self):
        g.local.http = True
        try:
            return _SAVED["connect"](self)
        finally:
            g.local.http = False

    def sconnect(self):
        ctx = self._context
        if ctx.verify_mode != ssl.CERT_REQUIRED or not ctx.check_hostname:
            g.refuse(getattr(self, "_jc_live", self.host), "TLS without certificate verification")
        return _SAVED["sconnect"](self)

    def create_connection(address, *a, **k):
        host, port = address[0], address[1]
        if not getattr(g.local, "http", False):
            g.refuse(f"socket {host}:{port}", "a raw socket: the live mode sends HTTP requests only")
        if str(host).lower() not in g.hosts:
            g.refuse(f"socket {host}:{port}", f"host {host} is not a host of {cfg['deployment']}")
        return _SAVED["create_connection"]((cfg["edge"], cfg["edge_port"]), *a, **k)

    def edge_only(addr):
        if not (getattr(g.local, "http", False) and isinstance(addr, tuple)
                and addr[:2] == (cfg["edge"], cfg["edge_port"])):
            g.refuse(f"socket {addr}", "only the edge, through an HTTP request")

    def sock_connect(self, addr):
        edge_only(addr)
        return _SAVED["sock_connect"](self, addr)

    def sock_connect_ex(self, addr):
        edge_only(addr)
        return _SAVED["sock_connect_ex"](self, addr)

    def popen(self, args, *a, **k):
        argv = [str(x) for x in ([args] if isinstance(args, (str, bytes)) else list(args))]
        if argv[:2] != [sys.executable, str(SMOKE / "run")]:
            g.refuse(f"exec {argv[:1]}", "only another smoke suite runs as a subprocess")
        return _SAVED["popen"](self, args, *a, **k)

    HC.putrequest, HC.putheader, HC.connect, HS.connect = putrequest, putheader, connect, sconnect
    socket.create_connection = create_connection
    socket.socket.connect, socket.socket.connect_ex = sock_connect, sock_connect_ex
    subprocess.Popen.__init__ = popen
    ACTIVE = g
    return g


def uninstall() -> None:
    global ACTIVE
    if ACTIVE is None:
        return
    HC, HS = http.client.HTTPConnection, http.client.HTTPSConnection
    HC.putrequest, HC.putheader, HC.connect = _SAVED["putrequest"], _SAVED["putheader"], _SAVED["connect"]
    HS.connect = _SAVED["sconnect"]
    socket.create_connection = _SAVED["create_connection"]
    socket.socket.connect, socket.socket.connect_ex = _SAVED["sock_connect"], _SAVED["sock_connect_ex"]
    subprocess.Popen.__init__ = _SAVED["popen"]
    ACTIVE = None


def install_from_env() -> Guard | None:
    raw = os.environ.get(ENV)
    return install(json.loads(raw)) if raw else None


# ------------------------------------------------------------------------------------ evidence

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
PERSON_KEYS = re.compile(
    r'("(?:username|preferred_username|email|firstName|lastName|given_name|family_name|name_full|fullName|'
    r'displayName|sub|userId|user_id|createdBy|creator_user_id|author|author_email|maintainer|'
    r'maintainer_email|owner)"\s*:\s*)"[^"]*"?')


def redact(text: str) -> str:
    """Person data out of a kept body: e-mail addresses and the values of person fields."""
    return PERSON_KEYS.sub(r'\1"<redacted>"', EMAIL.sub("<email>", str(text)))
