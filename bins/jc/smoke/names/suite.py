"""Smoke suite `names`: every name production serves, compared with jc's (owner goal GOAL-jc-dev:
the all-Rust copy keeps every production name; a rename only where RENAMES.md lists it and the old
address still answers).

   bins/jc/smoke/run names --base-url <url> --export <bundle> --out <dir> [--state <jc state>]
       [--renames <RENAMES.md>] [--prod-tools <prod-tools.json>] [--realm-secrets-dir <dir>]

Production's names come from the export alone (prodnames.py) and from --prod-tools, which
fetch_prod_tools.py writes by hand once: this suite never contacts prod or dev. Per kind:

  project        Portal GET /api/v1/projects (a signed-in test administrator)
  space          GET /api/v1/projects/{p}/spaces, as {p}/{space}
  endpoint-name  GET /api/v1/projects/{p}/endpoints, as {p}/{name}
  endpoint       the slug of each, from the same listing
  surface        /api/endpoint/{slug}/<surface path> of each enabled surface (NGSI-LD, OData, OGC,
                 STA, WebSocket, MCP) answers anything but 404 (an unknown slug's must be 404)
  mcp-server     /api/mcp and each McpServer's /api/mcp/{project}/{name} answer initialize
  tool           anonymous tools/list per server against prod's (public reads only), and the
                 write and admin tool names of prod's pinned source against this clone's
  dataset        /opendata/api/3/action/package_list (CKAN dataset names)
  app, dashboard GET /api/v1/projects/{p}/apps, .../dashboards
  realm, client  /id/realms/{realm} and the realm's client ids (only those)
  connector      skipped: /c/{id} is runtime state in the Portal database, no read-only source

A prod name missing on jc fails; an extra jc name is a report line (REPORT.md "Extra on jc");
RENAMES.md (| kind | old | new | why | how old clients keep working |) admits a difference only
when jc has the new name and the old address answers."""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import secrets
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
REPO = SMOKE.parent.parent.parent
for p in (HERE, SMOKE, SMOKE / "pipelines"):
    sys.path.insert(0, str(p))

import compare as cmp
import prodnames
import sources
from common import FAIL, PASS, SKIP, Http, Results, as_json, loopback_only, short

PROTOCOL = "2025-06-18"
MCP_HEADERS = {"Accept": "application/json, text/event-stream", "MCP-Protocol-Version": PROTOCOL,
               "Content-Type": "application/json"}
INIT = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "jc-smoke-names", "version": "1"}}}
CONNECTORS = "runtime state in the Portal database, no read-only source"
PUBLIC_ONLY = "public reads only (anonymous tools/list; the gateway lists read tools only, T-4168)"


def q(s: str) -> str:
    return urllib.parse.quote(s, safe="")


def parse_rpc(raw: bytes):
    """A JSON-RPC answer from a JSON body or the last `data:` event of an SSE body."""
    text = raw.decode(errors="replace").strip()
    if text[:1] in "{[":
        return as_json(raw)
    last = None
    for line in text.splitlines():
        if line.startswith("data:"):
            last = as_json(line[5:].strip().encode())
    return last


class Jc:
    """jc's side: anonymous reads through `http`, Portal reads through `portal` (a signed-in
    browser, or None when no administrator could be made)."""

    def __init__(self, http: Http, portal=None, workers: int = 8):
        self.http, self.portal, self.workers = http, portal, workers

    # ------------------------------------------------------------------ Portal listings
    def portal_json(self, path: str):
        if self.portal is None:
            return None, "no signed-in administrator"
        st, _, raw = self.portal.call("GET", path, headers={"Accept": "application/json"})
        body = as_json(raw)
        return (body, "") if st == 200 and body is not None else (None, f"{st} {short(raw, 100)}")

    def projects(self):
        body, why = self.portal_json("/api/v1/projects")
        if body is None:
            return None, why
        return {str(x.get("name")) for x in body.get("items") or [] if isinstance(x, dict)}, ""

    def resources(self, projects, plural: str):
        """{project/name: manifest} of one kind over the projects, or None and why."""
        out = {}
        for p in sorted(projects):
            body, why = self.portal_json(f"/api/v1/projects/{q(p)}/{plural}")
            if body is None:
                if why.startswith("404"):
                    continue  # a project jc lacks: its names are missing, not unreadable
                return None, f"{plural} of {p}: {why}"
            for m in body.get("items") or []:
                name = ((m or {}).get("metadata") or {}).get("name")
                if name:
                    out[f"{p}/{name}"] = m
        return out, ""

    def portal_answers(self, path: str):
        if self.portal is None:
            return False, "no signed-in administrator"
        st, _, _ = self.portal.call("GET", path, headers={"Accept": "application/json"})
        return st == 200, f"GET {path} {st}"

    # ------------------------------------------------------------------ addresses
    def status(self, method: str, path: str) -> int:
        if method == "POST":
            st, _, _ = self.http.call("POST", path, headers=MCP_HEADERS, body=INIT)
        else:
            st, _, _ = self.http.call("GET", path, headers={"Accept": "application/json"})
        return st

    def statuses(self, calls: list[tuple[str, str]]) -> dict:
        with cf.ThreadPoolExecutor(self.workers) as pool:
            return dict(zip(calls, pool.map(lambda c: self.status(*c), calls)))

    def tools(self, path: str):
        """Anonymous tools/list names of one MCP door, every page, or None and why."""
        st, h, raw = self.http.call("POST", path, headers=MCP_HEADERS, body=INIT)
        if st != 200:
            return None, f"initialize {st} {short(raw, 80)}"
        headers = dict(MCP_HEADERS)
        if h.get("mcp-session-id"):
            headers["Mcp-Session-Id"] = h["mcp-session-id"]
        names, cursor = set(), None
        for n in range(2, 22):
            msg = {"jsonrpc": "2.0", "id": n, "method": "tools/list", "params": {"cursor": cursor} if cursor else {}}
            st, _, raw = self.http.call("POST", path, headers=headers, body=msg)
            result = (parse_rpc(raw) or {}).get("result") if st == 200 else None
            if not isinstance(result, dict):
                return None, f"tools/list {st} {short(raw, 80)}"
            names |= {t["name"] for t in result.get("tools") or [] if isinstance(t, dict) and t.get("name")}
            cursor = result.get("nextCursor")
            if not cursor:
                break
        return names, ""

    def datasets(self):
        st, _, raw = self.http.call("GET", "/opendata/api/3/action/package_list", headers={"Accept": "application/json"})
        body = as_json(raw)
        if st != 200 or not isinstance(body, dict) or not isinstance(body.get("result"), list):
            return None, f"{st} {short(raw, 100)}"
        return {str(x) for x in body["result"]}, ""

    def realm(self, name: str):
        path = f"/id/realms/{q(name)}/.well-known/openid-configuration"
        st, _, raw = self.http.call("GET", path, headers={"Accept": "application/json"})
        return st == 200 and isinstance(as_json(raw), dict), f"GET {path} {st}"

    def clients(self, realm: str, token: str | None):
        """The realm's client ids through the admin API, or None and why."""
        if not token:
            return None, "no admin token for the realm"
        out, first = set(), 0
        while True:
            path = f"/id/admin/realms/{q(realm)}/clients?first={first}&max=500&briefRepresentation=true"
            st, _, raw = self.http.call("GET", path, token=token, headers={"Accept": "application/json"})
            body = as_json(raw)
            if st != 200 or not isinstance(body, list):
                return None, f"GET {path} {st} {short(raw, 80)}"
            out |= {c["clientId"] for c in body if isinstance(c, dict) and c.get("clientId")}
            if len(body) < 500:
                return out, ""
            first += 500


def realm_token(http: Http, realm: str, export_realm: dict, secrets_dir: Path | None, state: Path | None):
    """A token that may read `realm`'s clients: jc's own administrator client of the Portal realm
    (portal-reconciler, --state) when the realm is jc's, else a client of the imported realm whose
    secret the rehearsal made (--realm-secrets-dir, `$(env:NAME)` -> <dir>/NAME). Answers
    (token or None, which client)."""
    tries = []
    if state and (state / "portal-admin-client-secret").exists():
        tries.append(("jc", "portal-reconciler", (state / "portal-admin-client-secret").read_text().strip()))
    for c in export_realm.get("clients") or []:
        sec = str(c.get("secret") or "")
        if secrets_dir and c.get("serviceAccountsEnabled") and sec.startswith("$(") and sec.endswith(")"):
            f = secrets_dir / sec[2:-1].split(":")[-1]
            if f.exists() and c.get("clientId") == "portal-reconciler":
                tries.insert(0, (realm, c["clientId"], f.read_text().strip()))
    for in_realm, client, secret in tries:
        form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": client,
                                       "client_secret": secret}).encode()
        st, _, raw = http.call("POST", f"/id/realms/{q(in_realm)}/protocol/openid-connect/token",
                               headers={"Content-Type": "application/x-www-form-urlencoded"}, body=form)
        tok = (as_json(raw) or {}).get("access_token") if st == 200 else None
        if tok:
            st, _, _ = http.call("GET", f"/id/admin/realms/{q(realm)}/clients?max=1", token=tok)
            if st == 200:
                return tok, f"{in_realm}/{client}"
    return None, ", ".join(f"{r}/{c}" for r, c, _ in tries) or "no client secret"


# ---------------------------------------------------------------------- the run

def run(a, prod: dict, jc: Jc, res: Results, renames, prod_tools: dict | None, export: Path) -> dict:
    """Every kind's rows; answers {kind: extra jc names}."""
    extra = {}
    cmp.stale_renames(res, renames, {
        "project": set(prod["projects"]), "space": set(prod["spaces"]),
        "endpoint": {e.slug for e in prod["endpoints"]},
        "endpoint-name": {f"{e.project}/{e.name}" for e in prod["endpoints"]},
        "dataset": set(prod["datasets"]), "app": set(prod["apps"]), "dashboard": set(prod["dashboards"]),
        "mcp-server": {"/api/mcp", *(s["path"] for s in prod["mcpServers"])}, "realm": set(prod["realms"])})
    renamed = {r.kind: {} for r in renames}
    for r in renames:
        renamed[r.kind][r.old] = r.new

    # ---- projects and everything the Portal lists per project
    projects, why = jc.projects()
    extra["project"] = cmp.compare(res, "project", prod["projects"], projects, renames,
                                   lambda old: jc.portal_answers(f"/api/v1/projects/{q(old)}"),
                                   "GET /api/v1/projects", why)
    asked = set(prod["projects"]) | set(projects or ()) | set(renamed.get("project", {}).values())

    def portal_kind(kind, plural, prod_names, field=None):
        listing, why = jc.resources(asked, plural) if projects is not None else (None, "no project listing")
        names = set(listing) if listing is not None else None

        def old_answers(old):
            p, _, n = old.partition("/")
            return jc.portal_answers(f"/api/v1/projects/{q(p)}/{plural}/{q(n)}")
        extra[kind] = cmp.compare(res, kind, prod_names, names, renames, old_answers,
                                  f"GET /api/v1/projects/{{p}}/{plural}", why)
        return listing

    portal_kind("space", "spaces", prod["spaces"])
    ep_listing = portal_kind("endpoint-name", "endpoints", [f"{e.project}/{e.name}" for e in prod["endpoints"]])
    jc_slugs = None if ep_listing is None else {
        str((m.get("spec") or {}).get("slug")) for m in ep_listing.values() if (m.get("spec") or {}).get("slug")}
    slug_calls = {e.slug: ("GET", f"/api/endpoint/{e.slug}{prodnames.SURFACES['ngsi-ld'][1]}") for e in prod["endpoints"]}
    for old, new in renamed.get("endpoint", {}).items():
        slug_calls[old] = ("GET", f"/api/endpoint/{old}{prodnames.SURFACES['ngsi-ld'][1]}")
    portal_kind("app", "apps", prod["apps"])
    portal_kind("dashboard", "dashboards", prod["dashboards"])

    # ---- Endpoint slugs and their surface paths (anonymous; a closed Endpoint answers 401)
    control = "smoke-names-" + secrets.token_hex(6)
    surface_calls = {}
    for e in prod["endpoints"]:
        for rep in e.representations:
            if rep in prodnames.SURFACES:
                m, suffix = prodnames.SURFACES[rep]
                surface_calls[f"/api/endpoint/{e.slug}{suffix}"] = (m, f"/api/endpoint/{e.slug}{suffix}")
    for old, new in renamed.get("surface", {}).items():
        surface_calls.setdefault(old, ("GET", old))
        surface_calls.setdefault(new, ("GET", new))
    controls = {rep: (m, f"/api/endpoint/{control}{suffix}") for rep, (m, suffix) in prodnames.SURFACES.items()}
    got = jc.statuses(sorted(set(surface_calls.values()) | set(controls.values()) | set(slug_calls.values())))
    bad_controls = {rep: got[c] for rep, c in controls.items() if got[c] != 404}
    for rep, c in controls.items():
        res.check("surface.control", rep, f"{c[0]} {c[1]}", "404 for a slug no Endpoint has", str(got[c]),
                  rep not in bad_controls, "without it an answering path proves no name")

    def answered(call):
        return got.get(call, 0) not in (0, 404)

    jc_slugs_answering = {s for s, c in slug_calls.items() if answered(c)}
    extra["endpoint"] = cmp.compare(
        res, "endpoint", [e.slug for e in prod["endpoints"]],
        None if jc_slugs is None else (jc_slugs & jc_slugs_answering) | (jc_slugs - set(slug_calls)), renames,
        lambda old: (answered(slug_calls[old]), f"{' '.join(slug_calls[old])} {got.get(slug_calls[old])}"),
        "spec.slug listed and GET /api/endpoint/{slug}/ngsi-ld/v1/types not 404",
        "no endpoint listing")
    ok_surfaces = {p for p, c in surface_calls.items() if answered(c) and p.split("/")[3] != control}
    surface_rep = {p: rep for e in prod["endpoints"] for rep in e.representations if rep in prodnames.SURFACES
                   for p in [f"/api/endpoint/{e.slug}{prodnames.SURFACES[rep][1]}"]}
    unproven = {p for p, rep in surface_rep.items() if rep in bad_controls}
    jc_surface_names = None
    if ep_listing is not None:
        jc_surface_names = set()
        for m in ep_listing.values():
            s = m.get("spec") or {}
            for rep in s.get("enabledRepresentations") or []:
                if rep in prodnames.SURFACES:
                    jc_surface_names.add(f"/api/endpoint/{s.get('slug')}{prodnames.SURFACES[rep][1]}")
    listed_ok = (ok_surfaces - unproven) | ((jc_surface_names or set()) - set(surface_calls))
    extra["surface"] = cmp.compare(
        res, "surface", sorted(surface_calls if not renamed.get("surface") else set(surface_rep)),
        listed_ok, renames,
        lambda old: (answered(surface_calls[old]), f"{' '.join(surface_calls[old])} {got.get(surface_calls[old])}"),
        "the surface path answers (not 404)")

    # ---- MCP doors
    servers = {"/api/mcp": None, **{s["path"]: s for s in prod["mcpServers"]}}
    mcp_calls = {p: ("POST", p) for p in servers}
    for old, new in renamed.get("mcp-server", {}).items():
        mcp_calls.setdefault(old, ("POST", old))
        mcp_calls.setdefault(new, ("POST", new))
    mgot = jc.statuses(sorted(set(mcp_calls.values())))
    m_ok = {p for p, c in mcp_calls.items() if mgot[c] not in (0, 404)}
    extra["mcp-server"] = cmp.compare(
        res, "mcp-server", sorted(servers), m_ok, renames,
        lambda old: (mgot[mcp_calls[old]] not in (0, 404), f"POST {old} initialize {mgot[mcp_calls[old]]}"),
        "POST initialize answers (not 404)")
    tools(res, jc, prod_tools, sorted(servers), renames, a.repo)

    # ---- the catalogue, the realm, its clients
    ds, why = jc.datasets()
    extra["dataset"] = cmp.compare(
        res, "dataset", prod["datasets"], ds, renames,
        lambda old: (jc.http.call("GET", f"/opendata/api/3/action/package_show?id={q(old)}")[0] == 200,
                     f"package_show {old}"), "GET /opendata/api/3/action/package_list", why)
    jc_realms = {r for r in [*prod["realms"], *renamed.get("realm", {}).values()] if jc.realm(r)[0]}
    extra["realm"] = cmp.compare(res, "realm", sorted(prod["realms"]), jc_realms, renames, jc.realm,
                                 "GET /id/realms/{realm}/.well-known/openid-configuration")
    realm_files = {}
    for f in (export / "keycloak").glob("realm-*.json"):
        doc = json.loads(f.read_text())
        realm_files[doc.get("realm")] = doc
    for realm, clients in sorted(prod["realms"].items()):
        target = renamed.get("realm", {}).get(realm, realm)
        tok, who = realm_token(jc.http, target, realm_files.get(realm) or {},
                               Path(a.realm_secrets_dir) if a.realm_secrets_dir else None,
                               Path(a.state) if a.state else None)
        listed, why = jc.clients(target, tok)
        if listed is None and not why.startswith("no admin token"):
            why = f"{why} (as {who})"
        elif listed is None:
            why = f"{why}: tried {who}"
        listed_names = None if listed is None else {f"{realm}/{c}" for c in listed}
        extra["client"] = cmp.compare(
            res, "client", [f"{realm}/{c}" for c in clients], listed_names, renames,
            lambda old, listed=listed: (listed is not None and old.split("/", 1)[1] in listed, "client id still in the realm"),
            f"GET /id/admin/realms/{target}/clients", why)
    res.skip("connector", "/c/{id}", "", "every prod connector id answers on jc", CONNECTORS)
    return extra


def tools(res, jc: Jc, prod_tools: dict | None, servers: list[str], renames, repo: Path) -> None:
    if prod_tools is None:
        for s in servers:
            cmp.skip_kind(res, "tool", s, "tools/list", "no --prod-tools: run fetch_prod_tools.py once by hand")
        cmp.skip_kind(res, "tool", "source", "git show", "no --prod-tools: run fetch_prod_tools.py once by hand")
        return
    for s in servers:
        want = (prod_tools.get("public") or {}).get(s) or {}
        if want.get("tools") is None:
            cmp.skip_kind(res, "tool", s, "tools/list", f"prod-tools.json has no tool list of {s}: {want.get('detail', 'not fetched')}")
            continue
        have, why = jc.tools(s)
        names = None if have is None else {f"{s}#{t}" for t in have}
        cmp.compare(res, "tool", [f"{s}#{t}" for t in want["tools"]], names, renames,
                    lambda old, have=have: (have is not None and old.split("#", 1)[1] in have, "the old name is still listed"),
                    f"POST {s} tools/list (anonymous)", why, PUBLIC_ONLY)
    src = (prod_tools.get("source") or {}).get("tools") or {}
    mine = sources.jc_source_tools(repo)
    for part in ("gateway", "anonymous", "broker"):
        pin = (prod_tools.get("source") or {}).get("broker" if part == "broker" else "gateway") or {}
        cmp.compare(res, "tool", [f"source:{part}#{t}" for t in src.get(part) or []],
                    {f"source:{part}#{t}" for t in mine[part]}, renames,
                    lambda old, part=part: (old.split("#", 1)[1] in mine[part], "still in this clone's source"),
                    f"{pin.get('file')} at {pin.get('commit')} vs this clone",
                    note="write and admin names from prod's pinned source, read with git show")


def write_extra(out: Path, extra: dict) -> None:
    doc = json.loads((out / "results.json").read_text())
    doc["extra"] = {k: v for k, v in extra.items() if v}
    (out / "results.json").write_text(json.dumps(doc, indent=1, ensure_ascii=False))
    lines = ["", "## Extra on jc (report lines, no failure)", ""]
    if not doc["extra"]:
        lines.append("None.")
    for k, v in doc["extra"].items():
        lines.append(f"- {k} ({len(v)}): " + ", ".join(f"`{x}`" for x in v[:50]) + (" ..." if len(v) > 50 else ""))
    with (out / "REPORT.md").open("a") as fh:
        fh.write("\n".join(lines) + "\n")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="run names", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--export", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--state", help="jc state dir: portal-admin-client-secret (the test administrator)")
    ap.add_argument("--admin-token-file", help="unused: the suite signs in a test administrator")
    ap.add_argument("--person-token-file", help="unused by this suite")
    ap.add_argument("--renames", help="RENAMES.md: | kind | old | new | why | how old clients keep working |")
    ap.add_argument("--prod-tools", help="prod-tools.json of fetch_prod_tools.py")
    ap.add_argument("--realm-secrets-dir", help="the rehearsal's realm secrets (secrets/realm): the realm's client list")
    ap.add_argument("--repo", default=str(REPO), help="the clone jc was built from (its tool sources)")
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--workers", type=int, default=8)
    a = ap.parse_args(argv)
    loopback_only(a.base_url)
    a.repo = Path(a.repo)
    export = Path(a.export)
    renames = cmp.load_renames(a.renames)
    prod_tools = json.loads(Path(a.prod_tools).read_text()) if a.prod_tools else None
    prod = prodnames.load(export)
    res = Results("names", a.base_url)
    http = Http(a.base_url, timeout=a.timeout)
    admin = None
    portal = None
    if a.state:
        from people import Admin
        try:
            admin = Admin(a.base_url, Path(a.state))
            admin.username = "smoke-names-" + secrets.token_hex(3)
            admin.sign_in()
            portal = admin.browser
            res.check("session", "portal", "sign in a test administrator", "signed in", "signed in", True)
        except (RuntimeError, OSError, KeyError, IndexError) as e:
            res.check("session", "portal", "sign in a test administrator", "signed in", str(e)[:200], False)
    else:
        res.skip("session", "portal", "sign in a test administrator", "signed in", "no --state")
    try:
        extra = run(a, prod, Jc(http, portal, a.workers), res, renames, prod_tools, export)
    finally:
        if admin:
            admin.close()
    out = Path(a.out)
    t = res.totals()
    res.write(out, {"export": str(export), "renames": a.renames or "none", "prodTools": a.prod_tools or "none",
                    "prod": {k: len(v) for k, v in prod.items()}})
    write_extra(out, extra)
    print(json.dumps({"pass": t[PASS], "fail": t[FAIL], "skip": t[SKIP], "out": a.out}))
    return 1 if t[FAIL] else 0


if __name__ == "__main__":
    sys.exit(main())
