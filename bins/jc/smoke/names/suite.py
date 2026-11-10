"""Smoke suite `names`: every name production serves, compared with jc's (owner goal GOAL-jc-dev:
the all-Rust copy keeps every production name; a rename only where RENAMES.md lists it and the old
address still answers).

   bins/jc/smoke/run names --base-url <url> --export <bundle> --out <dir> [--state <jc state>]
       [--renames <RENAMES.md>] [--prod-tools <prod-tools.json>]

Production's names come from the export alone (prodnames.py) and from --prod-tools, which
fetch_prod_tools.py writes by hand once: this suite never contacts prod or dev. Per kind:

  project        Portal GET /api/v1/projects (a signed-in test administrator)
  space          GET /api/v1/projects/{p}/spaces, as {p}/{space}
  endpoint-name  GET /api/v1/projects/{p}/endpoints, as {p}/{name}
  endpoint       GET /api/endpoint/{slug} answers (an unknown slug's answer is 404)
  surface        /api/endpoint/{slug}/<surface path> of each enabled surface (NGSI-LD, OData, OGC,
                 STA, WebSocket, MCP) answers, and otherwise than an unknown slug
  mcp-server     /api/mcp and each McpServer's /api/mcp/{project}/{name} answer initialize
  tool           anonymous tools/list per server against prod's (public reads only), and the
                 write and admin tool names of prod's pinned source against this clone's
  dataset        /opendata/api/3/action/package_list (CKAN dataset names)
  app, dashboard GET /api/v1/projects/{p}/apps, .../dashboards
  realm, client  /id/realms/{realm}, and each prod client id of it is known: an anonymous
                 authorization request without redirect is refused otherwise than an unknown id
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
import time
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

    def client_answer(self, realm: str, client: str) -> tuple[int, str]:
        """(status, OAuth error) of an authorization request naming `client` and no redirect: an
        unknown client id gets the realm's `invalid_client`, a known one another refusal (missing
        redirect_uri, flow disabled). Read-only and anonymous: nothing signs in."""
        path = (f"/id/realms/{q(realm)}/protocol/openid-connect/auth?client_id={q(client)}"
                "&response_type=code&scope=openid")
        st, _, raw = self.http.call("GET", path, headers={"Accept": "application/json"})
        body = as_json(raw)
        return st, str(body.get("error")) if isinstance(body, dict) else short(raw, 40)

    def client_answers(self, realm: str, clients: list[str]) -> dict:
        with cf.ThreadPoolExecutor(self.workers) as pool:
            return dict(zip(clients, pool.map(lambda c: self.client_answer(realm, c), clients)))


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
    if getattr(a, "anonymous", False) and getattr(jc, "portal", None) is None:
        ep_listing = None
        for kind, n in (("project", prod["projects"]), ("space", prod["spaces"]), ("endpoint-name", prod["endpoints"]),
                        ("app", prod["apps"]), ("dashboard", prod["dashboards"])):
            res.skip(kind, f"{len(n)} prod names", "GET /api/v1/projects/...", "listed on jc",
                     "--anonymous: the Portal lists them to a signed-in person only (rehearsal, or a read-only "
                     "test person on the deployment)")
    else:
        projects, why = jc.projects()
        # A Portal that just started lists nothing until its first sync of the configuration
        # repositories ends: wait (bounded) for the projects to appear, then compare what is there.
        deadline = time.monotonic() + getattr(a, "settle", 0)
        while projects is not None and not projects & set(prod["projects"]) and time.monotonic() < deadline:
            time.sleep(5)
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
        portal_kind("app", "apps", prod["apps"])
        portal_kind("dashboard", "dashboards", prod["dashboards"])
    endpoints_and_surfaces(res, jc, prod, renames, renamed, ep_listing, extra)

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
    for realm, clients in sorted(prod["realms"].items()):
        target = renamed.get("realm", {}).get(realm, realm)
        control = "smoke-names-" + secrets.token_hex(6)
        news = [r.new.split("/", 1)[1] for r in renames if r.kind == "client" and r.old.startswith(f"{realm}/")]
        got = jc.client_answers(target, [control, *clients, *news])
        unknown = got.pop(control)
        res.check("client.control", target, f"GET /id/realms/{target}/protocol/openid-connect/auth?client_id=<unknown>",
                  "the realm's refusal of an unknown client", f"{unknown[0]} {unknown[1]}",
                  unknown[0] not in (0, 200, 404), "without it an answering client id proves nothing")
        known = None if unknown[0] in (0, 200, 404) else {f"{realm}/{c}" for c, ans in got.items() if ans != unknown}
        extra["client"] = cmp.compare(
            res, "client", [f"{realm}/{c}" for c in clients], known, renames,
            lambda old, got=got, unknown=unknown: (got.get(old.split("/", 1)[1], unknown) != unknown,
                                                   "the old client id is still known"),
            f"GET /id/realms/{target}/protocol/openid-connect/auth?client_id={{id}} (not {unknown[0]} {unknown[1]})",
            f"the realm answers an unknown client {unknown[0]}", "no listing: extra client ids are not seen")
    res.skip("connector", "/c/{id}", "", "every prod connector id answers on jc", CONNECTORS)
    return extra


def endpoints_and_surfaces(res, jc: Jc, prod: dict, renames, renamed: dict, listing, extra: dict) -> None:
    """Each prod slug answers at /api/endpoint/{slug}, and each enabled surface at its path.

    An unknown slug (the control) must answer 404 there. Where it does not (MCP asks for a token
    before it looks the slug up, 401), a path answering the same proves nothing: a public
    Endpoint must answer otherwise (MCP initialize 200), a closed one is a skip row."""
    control = "smoke-names-" + secrets.token_hex(6)
    roots, paths, public = {}, {}, {}
    for e in prod["endpoints"]:
        roots[e.slug] = ("GET", f"/api/endpoint/{e.slug}")
        public[e.slug] = not e.closed
        for rep in e.representations:
            if rep in prodnames.SURFACES:
                m, suffix = prodnames.SURFACES[rep]
                paths[f"/api/endpoint/{e.slug}{suffix}"] = (m, f"/api/endpoint/{e.slug}{suffix}", e.slug, rep)
    for r in renames:
        if r.kind == "endpoint":
            for slug in (r.old, r.new):
                roots.setdefault(slug, ("GET", f"/api/endpoint/{slug}"))
        if r.kind == "surface":
            for path in (r.old, r.new):
                slug, rep = path.split("/")[3], surface_of(path)
                paths.setdefault(path, (prodnames.SURFACES.get(rep, ("GET",))[0], path, slug, rep))
    controls = {"": ("GET", f"/api/endpoint/{control}")}
    controls |= {rep: (m, f"/api/endpoint/{control}{suffix}") for rep, (m, suffix) in prodnames.SURFACES.items()}
    got = jc.statuses(sorted({*roots.values(), *((m, p) for m, p, _, _ in paths.values()), *controls.values()}))
    ctl = {rep: got[c] for rep, c in controls.items()}
    for rep, c in controls.items():
        res.check("surface.control", rep or "endpoint", f"{c[0]} {c[1]}", "404, or a refusal read around",
                  str(ctl[rep]), ctl[rep] not in (0, 200), "an unknown slug's answer; a path answering the same proves no name")

    def present(st: int, rep: str, slug: str):
        if st in (0, 404):
            return False
        if st == ctl[rep] and ctl[rep] != 404:
            return False if public.get(slug, True) else None
        return True

    slugs_ok = {s for s, c in roots.items() if present(got[c], "", s)}
    if listing is not None:
        listed = {str((m.get("spec") or {}).get("slug")) for m in listing.values()}
        slugs_ok |= listed - set(roots)
    extra["endpoint"] = cmp.compare(
        res, "endpoint", [e.slug for e in prod["endpoints"]], slugs_ok, renames,
        lambda old: (old in slugs_ok, f"GET /api/endpoint/{old} {got.get(roots.get(old))}"),
        "GET /api/endpoint/{slug} (an unknown slug answers 404)")
    verdict = {p: present(got[(m, p)], rep, slug) for p, (m, _, slug, rep) in paths.items()}
    renamed_surfaces = {r.old for r in renames if r.kind == "surface"}
    want = [p for p, (_, _, slug, _) in paths.items() if slug in public and (p in renamed_surfaces or p not in
            {r.new for r in renames if r.kind == "surface"})]
    for p in sorted(want):
        if verdict[p] is None:
            m, _, slug, rep = paths[p]
            res.skip("surface", p, f"{m} {p}", "the surface path answers",
                     f"closed Endpoint: {m} answers {ctl[rep]} to any slug without a token, as here")
    ok = {p for p, v in verdict.items() if v}
    if listing is not None:
        for m in listing.values():
            sp = m.get("spec") or {}
            for rep in sp.get("enabledRepresentations") or []:
                if rep in prodnames.SURFACES:
                    path = f"/api/endpoint/{sp.get('slug')}{prodnames.SURFACES[rep][1]}"
                    if path not in paths:
                        ok.add(path)
    extra["surface"] = cmp.compare(
        res, "surface", sorted(p for p in want if verdict[p] is not None), ok, renames,
        lambda old: (bool(verdict.get(old)), f"{paths[old][0]} {old} {got.get(paths[old][:2])}"),
        "the surface path answers (not 404, not an unknown slug's answer)")


def surface_of(path: str) -> str:
    rest = "/" + "/".join(path.split("/")[4:])
    return next((rep for rep, (_, suffix) in prodnames.SURFACES.items() if rest == suffix), "")


# Tools a component that stays beside jc on dev serves through jc's gateway (bins/jc/CUTOVER.md
# section 1, GOAL-jc-dev "Scope of jc on dev"): the gateway lists them only when it knows that
# component's address, which the docker rehearsal (no Kubernetes) has none of. Proven on dev.
KEPT_TOOLS = {
    "search_documents": "assistant (stays a separate component on dev; jc's gateway lists the tool "
                        "once JC_GATEWAY_ASSISTANT_URL names it, bins/jc/CUTOVER.md section 1)",
}


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
                    f"POST {s} tools/list (anonymous)", why, PUBLIC_ONLY,
                    kept={f"{s}#{t}": KEPT_TOOLS[t] for t in want["tools"] if t in KEPT_TOOLS})
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
    ap.add_argument("--repo", default=str(REPO), help="the clone jc was built from (its tool sources)")
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--anonymous", action="store_true",
                    help="no --state: the Portal-listing kinds are named skips instead of failures (live mode)")
    ap.add_argument("--settle", type=float, default=300,
                    help="seconds to wait for a fresh Portal's first sync to list a prod project")
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
