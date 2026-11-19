"""Live mode: the anonymous read parts of the smoke suites against one named jc deployment on a
cluster (jc-prod, jc-dev), through its APISIX edge, before or after DNS points at it.

`run all --deployment <name> --domain <d> --edge <ip> --jc-config <jc.toml>` puts the suites on a
virtual base `https://<name>.smoke.invalid` (a name that never resolves) and installs one guard in
its own process and in every suite process (`run` installs it from $JC_SMOKE_LIVE):

- Routing: a path on the virtual base goes to the host jc's host-mode mounts serve it on, as the
  rehearsal's slug mounts did (`/login/...` -> idm.<d>/..., `/opendata/...` -> opendata.<d>/...,
  the gateway's root paths -> its first host, anything else -> the Portal's host). Each request
  connects to the edge IP and names the real host in Host and SNI; TLS is verified.
- Hosts: only the domain's hosts the jc.toml mounts; any other host, a raw socket, an unverified
  TLS context and every subprocess but another `run` are refused.
- Outside hosts (suite `orgs`: an Organization's crawl and harvest sources, named by the caller):
  HEAD only, reached directly by their own DNS answer, refused when any address it names is not a
  public one (loopback, private, link-local: the cluster's or the runner's own network).
- Requests: GET, HEAD and OPTIONS; POST only to the anonymous MCP JSON-RPC paths, the anonymous
  write refusal probe and an assistant's visitor token and chat; never an Authorization or Cookie
  header. A refused request
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
SLUGS = {"kaisatsu": "login", "dataroom": "opendata", "daiya": "dashboards", "viz": "embed",
         "assistant": "assistant", "model-tools": "model-tools"}
# Module::root_paths of the gateway: it answers at these whole paths on every host.
GATEWAY_ROOTS = ("/api/endpoint", "/api/endpoints", "/api/mcp", "/cs", "/catalog", "/catalog.jsonld",
                 "/catalog.ttl", "/oauth", "/.well-known/oauth-authorization-server",
                 "/.well-known/oauth-protected-resource/api/mcp",
                 "/.well-known/oauth-protected-resource/api/endpoint")
READ_METHODS = {"GET", "HEAD", "OPTIONS"}
# POST, anonymous only: the MCP JSON-RPC doors and the gateway's anonymous write refusal probe.
MCP_PATHS = (re.compile(r"/api/mcp(/[^/?]+/[^/?]+)?/?"), re.compile(r"/api/endpoint/[^/?]+/mcp/?"))
# The visitor flow of a public AssistantDeployment (assistant chat/mod.rs): the proof's token and one question.
VISITOR_PATHS = (re.compile(r"/api/v1/d/[a-z0-9][a-z0-9-]{0,62}/(token|chat)"),)
POST_PATHS = (*MCP_PATHS, *VISITOR_PATHS, re.compile(r"/api/endpoint/[a-z2-7]+/ngsi-ld/v1/entities"))
# Behind the edge the gateway counts anonymous MCP per client address: 120 a minute, burst 30
# (context-gateway ANONYMOUS_MCP_RATE_LIMIT, spend_anonymous_mcp). A loopback rehearsal carries no
# X-Forwarded-For and is never counted, so the suites' own pacing is faster than that.
MCP_RATE = 1.8
SECRET_HEADERS = {"authorization", "cookie", "proxy-authorization"}
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,62}")
HOST = re.compile(r"[a-z0-9]([a-z0-9-]{0,62})(\.[a-z0-9]([a-z0-9-]{0,62}))+")


class Refused(ConnectionRefusedError):
    pass


def under(host: str, domain: str) -> bool:
    return "*" not in host and (host == domain or host.endswith("." + domain))


def routes(jc_toml: str, domain: str) -> tuple[list[tuple[str, str, str]], set[str]]:
    """([(virtual prefix, host, real prefix)] longest first, every host) of a host- or slug-mode jc.toml.
    Host mode: entries on a host outside `domain` (localhost:8080, a wildcard) are left out: never
    reached. Slug mode serves every module as paths of base_host, so the virtual paths go there
    whole; it is accepted only with [[organizations]] on hosts of their own (jc-dev), the hosts the
    orgs suite probes."""
    doc = tomllib.loads(jc_toml)
    if doc.get("mode") not in ("host", "slug"):
        raise SystemExit("--jc-config: neither a host-mode nor a slug-mode jc.toml (mode = \"host\" or \"slug\")")
    if doc.get("base_host", "").lower() != domain:
        raise SystemExit(f"--jc-config: base_host {doc.get('base_host')!r} is not --domain {domain!r}")
    if doc["mode"] == "slug":
        own = {h for o in doc.get("organizations") or [] if isinstance(o, dict) and o.get("host") != "auto"
               for h in (o.get("hosts") or {}).values() if isinstance(h, str) and h}
        if not own:
            raise SystemExit("--jc-config: a slug-mode jc.toml whose [[organizations]] name no hosts: no host to probe")
        return [("", domain, "")], plain_hosts(own, "an Organization host") | {domain}
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


def plain_hosts(hosts, what: str) -> set[str]:
    out = {str(h).lower().rstrip(".") for h in hosts}
    if bad := sorted(h for h in out if not HOST.fullmatch(h)):
        raise SystemExit(f"{what}: not a plain host name: {', '.join(bad)}")
    return out


def config(deployment: str, domain: str, edge: str, jc_toml: str, out: Path, rate: float = 5.0,
           edge_port: int = 443, scheme: str = "https", org_hosts=(), outside=()) -> dict:
    """`org_hosts`: further hosts of the deployment (an Organization's own domain), reached
    through the edge like the mounts'. `outside`: hosts the guard admits HEAD to, directly."""
    domain = domain.lower().rstrip(".")
    if not NAME.fullmatch(deployment):
        raise SystemExit(f"--deployment {deployment!r}: a lower-case name")
    if not re.fullmatch(r"[a-z0-9.-]+\.[a-z]{2,}", domain):
        raise SystemExit(f"--domain {domain!r}: a plain domain name")
    ipaddress.ip_address(edge)
    if rate <= 0:
        raise SystemExit("--rate: above 0")
    table, hosts = routes(jc_toml, domain)
    hosts |= plain_hosts(org_hosts, "an Organization host")
    outside = plain_hosts(outside, "an outside host") - hosts
    out.mkdir(parents=True, exist_ok=True)
    return {"deployment": deployment, "domain": domain, "edge": edge, "edge_port": edge_port,
            "scheme": scheme, "virtual": f"{deployment}.smoke.invalid", "routes": table,
            "hosts": sorted(hosts), "outside": sorted(outside), "rate": rate, "bucket": str(out / ".rate"),
            "log": str(out / "refused.jsonl")}


def base_url(cfg: dict) -> str:
    return f"{cfg['scheme']}://{cfg['virtual']}"


# ------------------------------------------------------------------------------------ guard

class Guard:
    def __init__(self, cfg: dict):
        self.cfg = cfg
        self.hosts = set(cfg["hosts"])
        self.outside = set(cfg.get("outside") or ())
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
        if host in self.hosts or host in self.outside:
            return host, path + query
        raise Refused(f"host {host or '?'} is not a host of {self.cfg['deployment']}")

    def method(self, method: str, path: str, host: str = "") -> None:
        if host in self.outside:
            if method == "HEAD":
                return
            raise Refused(f"{method} {host}: an outside host takes HEAD only")
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

    def wait(self, bucket: str | None = None, rate: float | None = None) -> float:
        """One slot of the shared rate: the bucket file holds the time the next request may go.
        The wait sleeps under the lock and the next slot counts from the wake, not the plan: a
        waiter that wakes late must not leave the next one less than a slot (smoke run
        42921231590: grants 50 ms apart, requests 19 ms apart). Returns the grant time, read
        under the lock: a clock read after the return can lag it by a preemption (run
        42983380153 read two grants 0.35 ms apart)."""
        with open(bucket or self.cfg["bucket"], "a+") as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            f.seek(0)
            try:
                nxt = float(f.read() or 0)
            except ValueError:
                nxt = 0.0
            time.sleep(max(0.0, nxt - time.time()))
            now = time.time()
            f.seek(0)
            f.truncate()
            f.write(repr(now + 1 / (rate or self.cfg["rate"])))
            return now


def resolve(host: str, port: int) -> list[tuple[str, int]]:
    """Every (address, port) DNS answers for an outside host, empty when it answers none."""
    try:
        return [(i[4][0], port) for i in socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)]
    except OSError:
        return []


def is_public(ip: str) -> bool:
    return ipaddress.ip_address(ip).is_global


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
            g.method(method, path, host)
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
        if str(host).lower() in g.outside:
            addrs = resolve(str(host).lower(), port)
            if not addrs or not all(is_public(ip) for ip, _ in addrs):
                g.refuse(f"socket {host}:{port}", f"outside host {host} names an address that is not public "
                         f"({', '.join(sorted({ip for ip, _ in addrs})) or 'none'})")
            g.local.dest = addrs[0]
            try:
                return _SAVED["create_connection"](addrs[0], *a, **k)
            finally:
                g.local.dest = None
        if str(host).lower() not in g.hosts:
            g.refuse(f"socket {host}:{port}", f"host {host} is not a host of {cfg['deployment']}")
        return _SAVED["create_connection"]((cfg["edge"], cfg["edge_port"]), *a, **k)

    def edge_only(addr):
        if not (getattr(g.local, "http", False) and isinstance(addr, tuple)
                and (addr[:2] == (cfg["edge"], cfg["edge_port"])
                     or (getattr(g.local, "dest", None) and addr[:2] == g.local.dest))):
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
