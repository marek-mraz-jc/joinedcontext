"""Smoke suite `orgs` (T-7309): each Organization of a live deployment, as it is configured now,
anonymous and read-only behind live.py's guard.

    bins/jc/smoke/run orgs --jc-config <jc.toml> --deployment <name> --domain <d> --edge <ip>
        --out <dir> [--snapshot <json>] [--assistant ORG=PUBLICID]... [--app-host ORG=HOST]...
        [--question <text>] [--max-crawl-hours 48] [--rate 2]

The Organizations are jc.toml's [[organizations]] (the seed) joined with the registry rows of the
snapshot (`watch.py` reads it read-only from the Portal's and the assistant's databases): a new
Organization, host, crawl source or harvest source is covered on the next run with no edit here.
Snapshot, every key optional:

    {"organizations": [{"name", "host", "catalogueId", "hosts": [{"host", "role"}]}],
     "knowledge": [{"organization", "project", "source", "root", "lastCrawl", "pending", "errors"}]}

Rows, per Organization O (target O; ids carry O so an incident is one Organization and one check):
- portal.O: its working host answers 2xx/3xx.
- oidc.O: its realm's discovery (O on hosts.idm, else the installation realm on the sign-in host)
  answers 200 over verified TLS and names exactly that issuer.
- prm.O: the protected-resource metadata of the Portal MCP on its working host (/api/v1/mcp, the
  Portal's door) names that host as the resource and that realm alone.
- prm.gateway: the gateway's own host (the first [[mounts.gateway]] without a path prefix) serves
  the metadata of /api/mcp naming itself as the resource and the installation realm alone.
- catalogue.O: its open data catalogue answers package_search (success true).
- mcp.O: MCP initialize and tools/list on its API host, anonymous, list at least one tool.
- app.O.<host>: each App host (--app-host) answers 2xx/3xx.
- harvest.O.<name>: each enabled [[dataroom.harvest]] source of O answers HEAD (any status < 500).
- crawl.O.<project>/<source>: each crawl root answers HEAD (< 500), and its last crawl is younger
  than --max-crawl-hours.
- embed.O: no passage of O's crawl sources holds an embedder refusal (`chunks.embed_error`); a
  skip on an assistant schema before migration 0025 (no such column: `errors` null).
- assistant.O: with --assistant O=<publicId>, one fixed question through the visitor flow answers
  (no `error` event, an `answer`) citing at least one page of O's own crawl roots.
An outside host (crawl and harvest) is HEAD only and only when DNS names public addresses alone.
Result lines carry statuses and counts, never a response body or a person's data."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import tomllib
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
for p in (SMOKE, SMOKE / "mcp"):
    sys.path.insert(0, str(p))

import live
from client import Mcp
from common import FAIL, Http, Results, as_json, events, solve

SUITE = "orgs"
NAME = re.compile(r"[a-z][a-z0-9_-]{0,62}")
QUESTION = "Čo nájdem na vašej stránke?"
ROLES = {"working": "portal", "opendata": "opendata", "api": "api", "signin": "idm", "apps": "apps"}


def pairs(values: list[str], what: str) -> dict[str, list[str]]:
    """{org: [value]} of repeated ORG=VALUE flags."""
    out: dict[str, list[str]] = {}
    for v in values:
        org, sep, val = v.partition("=")
        if not sep or not NAME.fullmatch(org) or not val:
            raise SystemExit(f"{what} {v!r}: ORG=VALUE")
        out.setdefault(org, []).append(val.strip().lower())
    return out


def build(doc: dict, snap: dict, assistants: dict | None = None, apps: dict | None = None) -> dict[str, dict]:
    """{org: what its rows probe}: jc.toml's seed, the registry's rows over it, the harvest seeds
    and crawl sources it owns. An Organization the registry marks removed is left out."""
    orgs: dict[str, dict] = {}

    def org(name: str) -> dict:
        return orgs.setdefault(name, {"hosts": {}, "catalogue": None, "harvest": [], "crawl": [],
                                      "assistant": None, "apps": []})

    for o in doc.get("organizations") or []:
        if not NAME.fullmatch(o.get("name") or "") or o.get("host") == "auto":
            continue
        e = org(o["name"])
        e["hosts"].update({k: v.lower() for k, v in (o.get("hosts") or {}).items()
                           if isinstance(v, str) and v and k != "mcp"})
        e["catalogue"] = o.get("catalogue_id")
    removed = set()
    for row in snap.get("organizations") or []:
        name = row.get("name") or ""
        if not NAME.fullmatch(name):
            continue
        if row.get("state", "active") != "active":
            removed.add(name)
            continue
        e = org(name)
        if row.get("host"):
            e["hosts"].setdefault("portal", row["host"].lower())
        for h in row.get("hosts") or []:
            key = ROLES.get(h.get("role") or "")
            if key and h.get("host"):
                e["hosts"][key] = h["host"].lower()
        e["catalogue"] = row.get("catalogueId") or e["catalogue"]
    for seed in (doc.get("dataroom") or {}).get("harvest") or []:
        if seed.get("organization") in orgs and seed.get("enabled", True):
            orgs[seed["organization"]]["harvest"].append({"name": seed["name"], "url": seed["url"]})
    for k in snap.get("knowledge") or []:
        if k.get("organization") in orgs:
            orgs[k["organization"]]["crawl"].append(k)
    for name, ids in (assistants or {}).items():
        if name in orgs:
            orgs[name]["assistant"] = ids[0]
    for name, hosts in (apps or {}).items():
        if name in orgs:
            orgs[name]["apps"] = sorted(set(hosts))
    for name in removed:
        orgs.pop(name, None)
    return {n: e for n, e in sorted(orgs.items()) if e["hosts"].get("portal")}


def host_of(url: str) -> str:
    return (urllib.parse.urlsplit(url).hostname or "").lower()


def outside_hosts(orgs: dict) -> set[str]:
    """The crawl and harvest hosts the guard admits HEAD to: the ones the configuration names."""
    return {h for e in orgs.values() for s in [*e["harvest"], *({"url": c.get("root") or ""} for c in e["crawl"])]
            if (h := host_of(s["url"]))}


def org_hosts(orgs: dict) -> set[str]:
    return {h for e in orgs.values() for k, h in e["hosts"].items() if k != "apps"} | \
        {h for e in orgs.values() for h in e["apps"]}


def parse_time(s: str | None) -> dt.datetime | None:
    try:
        return dt.datetime.fromisoformat(str(s)) if s else None
    except ValueError:
        return None


def answered(st: int) -> bool:
    return 200 <= st < 400


class Suite:
    def __init__(self, doc: dict, orgs: dict, res: Results, a):
        self.doc, self.orgs, self.r, self.a = doc, orgs, res, a
        self.scheme = a.scheme
        self.installation = (doc.get("identity") or {}).get("realm") or "jc"
        mount = (doc.get("mounts") or {}).get("kaisatsu")
        self.login = (mount.get("host") if isinstance(mount, dict) else None) or doc.get("base_host", "")
        mount = (doc.get("mounts") or {}).get("assistant")
        self.assistant_host = mount.get("host") if isinstance(mount, dict) else None
        mounts = (doc.get("mounts") or {}).get("gateway") or []
        self.gateway = next((m["host"].lower() for m in ([mounts] if isinstance(mounts, dict) else mounts)
                             if isinstance(m, dict) and m.get("host") and not m.get("path_prefix")), None)

    def http(self, host: str) -> Http:
        return Http(f"{self.scheme}://{host}", timeout=self.a.timeout)

    def run(self) -> None:
        if self.gateway:
            self.prm("prm.gateway", "gateway", self.gateway, "/api/mcp",
                     f"{self.scheme}://{self.login}/realms/{self.installation}")
        for o, e in self.orgs.items():
            self.one(o, e)

    def one(self, o: str, e: dict) -> None:
        hs = e["hosts"]
        portal, api = hs["portal"], hs.get("api") or hs["portal"]
        st, _, _ = self.http(portal).call("GET", "/")
        self.r.check(f"portal.{o}", o, f"GET {portal}/", "2xx/3xx", str(st), answered(st))

        signin, realm = (hs["idm"], o) if hs.get("idm") else (self.login, self.installation)
        issuer = f"{self.scheme}://{signin}/realms/{realm}"
        st, _, raw = self.http(signin).call("GET", f"/realms/{realm}/.well-known/openid-configuration")
        got = (as_json(raw) or {}).get("issuer") if st == 200 else None
        self.r.check(f"oidc.{o}", o, f"GET {signin}/realms/{realm}/.well-known/openid-configuration",
                     f"200, issuer {issuer}", f"{st} {got or ''}".strip(), st == 200 and got == issuer)

        self.prm(f"prm.{o}", o, portal, "/api/v1/mcp", issuer)

        if hs.get("opendata"):
            od, path = hs["opendata"], "/api/3/action/package_search?rows=0"
        else:
            od, path = portal, "/data/api/3/action/package_search?rows=0"
        st, _, raw = self.http(od).call("GET", path)
        ok = st == 200 and (as_json(raw) or {}).get("success") is True
        self.r.check(f"catalogue.{o}", o, f"GET {od}{path}", "200, success true", str(st), ok)

        self.mcp(o, api)
        for h in e["apps"]:
            st, _, _ = self.http(h).call("GET", "/")
            self.r.check(f"app.{o}.{h}", o, f"GET {h}/", "2xx/3xx", str(st), answered(st))
        for s in e["harvest"]:
            self.reach(f"harvest.{o}.{s['name']}", o, s["url"])
        now = dt.datetime.now(dt.UTC)
        for c in e["crawl"]:
            rid = f"crawl.{o}.{c.get('project')}/{c.get('source')}"
            if not c.get("root"):
                self.r.skip(rid, o, "the crawl root", "a root page", "no root page: an upload source or not crawled yet")
                continue
            last = parse_time(c.get("lastCrawl"))
            age = (now - last).total_seconds() / 3600 if last else None
            if not self.reach(rid, o, c["root"]):
                continue
            self.r.check(f"{rid}.fresh", o, "sites.last_crawl", f"under {self.a.max_crawl_hours} h",
                         f"{age:.1f} h" if age is not None else "never",
                         age is not None and age <= self.a.max_crawl_hours)
        if e["crawl"] and all(c.get("errors") is None for c in e["crawl"]):
            self.r.skip(f"embed.{o}", o, "chunks.embed_error of O's sources", "0 refused passages",
                        "the assistant schema has no chunks.embed_error yet (assistant migration 0025)")
        elif e["crawl"]:
            errors = sum(int(c.get("errors") or 0) for c in e["crawl"])
            pending = sum(int(c.get("pending") or 0) for c in e["crawl"])
            self.r.check(f"embed.{o}", o, "chunks.embed_error of O's sources", "0 refused passages",
                         f"{errors} refused, {pending} waiting", errors == 0)
        self.assistant(o, e)

    def reach(self, rid: str, o: str, url: str) -> bool:
        u = urllib.parse.urlsplit(url)
        path = (u.path or "/") + (f"?{u.query}" if u.query else "")
        st, _, raw = Http(f"{u.scheme}://{u.netloc}", timeout=self.a.timeout).call("HEAD", path)
        why = raw.decode(errors="replace")[:160] if st == 0 else ""
        ok = 0 < st < 500
        self.r.check(rid, o, f"HEAD {u.netloc}{u.path}", "an answer under 500", str(st), ok, why)
        return ok

    def prm(self, rid: str, o: str, host: str, door: str, issuer: str) -> None:
        """The protected-resource metadata of `door` on `host`: resource on that host, `issuer` the only realm."""
        path = f"/.well-known/oauth-protected-resource{door}"
        st, _, raw = self.http(host).call("GET", path)
        doc = (as_json(raw) if st == 200 else None) or {}
        resource = doc.get("resource") if isinstance(doc.get("resource"), str) else ""
        named = sorted({s.rstrip("/") for s in doc.get("authorization_servers") or []
                        if isinstance(s, str) and "/realms/" in s})
        own = host_of(resource) == host
        self.r.check(rid, o, f"GET {host}{path}", f"200, resource on {host}, naming {issuer} alone",
                     f"{st} {'' if own else 'resource ' + (host_of(resource) or 'missing') + ' '}{' '.join(named)}".strip(),
                     own and named == [issuer])

    def mcp(self, o: str, api: str) -> None:
        m = Mcp(self.http(api), "/api/mcp", retries=1)
        init = m.initialize()
        if not init.ok:
            self.r.check(f"mcp.{o}", o, f"POST {api}/api/mcp initialize", "initialize, tools/list",
                         f"initialize {init.status}", False)
            return
        ans, tools = m.tools()
        self.r.check(f"mcp.{o}", o, f"POST {api}/api/mcp initialize, tools/list", "at least one tool",
                     f"{ans.status}, {len(tools)} tools", ans.ok and len(tools) > 0)

    def assistant(self, o: str, e: dict) -> None:
        pid = e["assistant"]
        rid = f"assistant.{o}"
        if not pid:
            self.r.skip(rid, o, "POST /api/v1/d/<publicId>/chat", "an answer citing O's own pages",
                        f"no public assistant named for {o} (watch --assistant {o}=<publicId>)")
            return
        h = self.http(self.assistant_host or e["hosts"]["portal"])
        call = f"POST {h.base.split('://')[1]}/api/v1/d/{pid}/chat"
        st, _, raw = h.call("GET", f"/api/v1/d/{pid}/challenge")
        ch = as_json(raw) or {}
        if st != 200 or not ch.get("challenge"):
            self.r.check(rid, o, call, "a visitor token", f"challenge {st}", False)
            return
        nonce = solve(ch["challenge"], int(ch.get("difficulty") or 0))
        st, _, raw = h.call("POST", f"/api/v1/d/{pid}/token", body={"challenge": ch["challenge"], "nonce": nonce})
        tok = (as_json(raw) or {}).get("token") if st == 200 else None
        if not tok:
            self.r.check(rid, o, call, "a visitor token", f"token {st}", False)
            return
        st, _, raw = h.call("POST", f"/api/v1/d/{pid}/chat", body={"message": self.a.question},
                            headers={"X-JC-Visitor": tok, "Accept": "text/event-stream"})
        evs = events(raw) if st == 200 else []
        errors = [d.get("status") for n, d in evs if n == "error" and isinstance(d, dict)]
        cited = [c.get("url") for n, d in evs if n == "citations" and isinstance(d, list)
                 for c in d if isinstance(c, dict) and c.get("url")]
        own = {host_of(c.get("root") or "") for c in e["crawl"]} - {""}
        mine = [u for u in cited if not own or host_of(u) in own]
        has_answer = any(n == "answer" for n, _ in evs)
        actual = f"{st}, {'answer' if has_answer else 'no answer'}, {len(mine)}/{len(cited)} own citations" + \
            (f", error {errors}" if errors else "")
        self.r.check(rid, o, call, "an answer, no error event, a citation of O's own pages", actual,
                     st == 200 and has_answer and not errors and bool(mine))


def main(argv) -> int:
    p = argparse.ArgumentParser(prog="run orgs")
    p.add_argument("--out", required=True)
    p.add_argument("--base-url", help="ignored: the hosts come from --jc-config and --snapshot")
    p.add_argument("--jc-config", required=True, help="the deployment's host-mode jc.toml")
    p.add_argument("--deployment", required=True, help="its name, e.g. jc-prod")
    p.add_argument("--domain", required=True, help="its base_host, e.g. smartcity.joinedcontext.com")
    p.add_argument("--edge", required=True, help="the edge's IP address every host of the deployment reaches")
    p.add_argument("--edge-port", type=int, default=443)
    p.add_argument("--scheme", default="https", help=argparse.SUPPRESS)  # http: the tests' loopback edge
    p.add_argument("--snapshot", help="the registry and knowledge snapshot watch.py reads (JSON)")
    p.add_argument("--assistant", action="append", default=[], help="ORG=PUBLICID of O's public assistant")
    p.add_argument("--app-host", action="append", default=[], help="ORG=HOST of one of O's Apps")
    p.add_argument("--question", default=QUESTION)
    p.add_argument("--max-crawl-hours", type=float, default=48)
    p.add_argument("--rate", type=float, default=2.0, help="requests per second, all hosts together")
    p.add_argument("--timeout", type=float, default=30)
    a = p.parse_args(argv)
    out = Path(a.out)
    jc_toml = Path(a.jc_config).read_text()
    doc = tomllib.loads(jc_toml)
    snap = json.loads(Path(a.snapshot).read_text()) if a.snapshot else {}
    orgs = build(doc, snap, pairs(a.assistant, "--assistant"), pairs(a.app_host, "--app-host"))
    cfg = live.config(a.deployment, a.domain, a.edge, jc_toml, out, rate=a.rate, edge_port=a.edge_port,
                      scheme=a.scheme, org_hosts=org_hosts(orgs), outside=outside_hosts(orgs))
    if live.active():
        raise SystemExit("run orgs on its own (orgs/watch.py): it installs a guard of its own hosts")
    live.install(cfg)
    res = Results(SUITE, a.deployment)
    if not orgs:
        res.skip("organizations", a.deployment, "jc.toml and the registry", "an Organization",
                 "no Organization with a working host")
    try:
        Suite(doc, orgs, res, a).run()
    finally:
        live.uninstall()
    per = {o: {"checks": 0, "fail": 0} for o in orgs}
    for row in res.rows:
        if row["target"] in per:
            per[row["target"]]["checks"] += 1
            per[row["target"]]["fail"] += row["result"] == FAIL
    res.write(out, {"organizations": per})
    return 1 if res.totals()["fail"] else 0
