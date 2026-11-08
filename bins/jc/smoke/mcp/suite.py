"""Smoke suite `mcp`: the MCP connectors of a rehearsal, on the data its export holds.

1. The public hub `/api/mcp`, anonymously: initialize, tools/list (no write tool offered),
   list_endpoints (exactly the public Endpoints that serve mcp), and per public Endpoint
   query_entities (a page of its largest type, one exported entity by id, the count against the
   export), describe_schema, query_temporal over the last 24 h (the exported instance) and before
   it (nothing), search, and a write that must be refused and leave nothing behind.
2. An Endpoint's own MCP `/api/endpoint/{slug}/mcp`, one Endpoint per class (audience x data
   shape): the same reads without `endpoint`, the write refused; a closed Endpoint's door 401s.
3. The Portal's MCP `/api/v1/mcp` as the three administrators of MCP-ADMIN.md (platform,
   organization, project), each a person of the rehearsal with a RoleBinding and an `mcp-local`
   token: what each is listed, the project list each reads, the installation readings, a manifest
   of the export by name, a data query through the Endpoint, and the calls each is refused.
4. A connector `/c/{id}` the project administrator creates over one Endpoint: no token and a
   Portal token 401, its data query answers the exported entity, a call outside its scope is
   refused, and after the revoke the connector answers nothing.

Parts 3 and 4 need `--state <rehearsal state dir>` (the realm admin client's secret and the shako
token); without it they are skipped with that reason. Every name the suite writes is
`smoke-mcp-*` and is removed at the end (`--keep` leaves it)."""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import datetime as dt
import json
import secrets
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.append(str(HERE.parent / "endpoints"))

import export as ex  # noqa: E402
from client import WRITE_TOOLS, Mcp, entities_of, text_of, total_of  # noqa: E402
from common import Http, Results, loopback_only, read_token  # noqa: E402

HUB_TOOLS = {"initialize"}
HUB_READS = {"search", "list_endpoints", "get_endpoint", "query_entities", "query_temporal", "describe_schema"}
ADMIN_TOOLS = {"jc_organization_read", "jc_access_request_list", "jc_access_request_decide",
               "jc_serviceaccount_access"}
ORG_NAMESPACE = "org"
SMOKE_TYPE = "SmokeMcpProbe"


def iso(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def ids_in(data) -> set[str]:
    return {str(e.get("id")) for e in entities_of(data) if e.get("id")}


def instances(data) -> int:
    """Temporal instances in a query_temporal answer, whatever the representation."""
    n = 0
    for e in entities_of(data):
        for k, v in e.items():
            if k in ("id", "type", "@context", "scope"):
                continue
            if isinstance(v, list):
                n += sum(1 for x in v if isinstance(x, (dict, list)))
            elif isinstance(v, dict):
                n += len(v.get("values") or []) if "values" in v else 1
    return n


# ------------------------------------------------------------------------------------ export

class Expect:
    """What the export says the MCP doors must answer."""

    def __init__(self, export: Path, cache: Path, log=print):
        self.export = export
        files = export / "gitea" / "files"
        self.endpoints = ex.load_endpoints(files)
        self.index = ex.load_index(export, self.endpoints, cache, log=log)
        self.orgs, self.projects, self.installation = {}, {}, None
        for f in files.glob("*/*/organizations/*.yaml"):
            for d in ex._load_yaml(f):
                if d.get("kind") == "Organization":
                    self.orgs[d["metadata"]["name"]] = d.get("spec") or {}
                    if (d.get("spec") or {}).get("installation"):
                        self.installation = d["metadata"]["name"]
        for f in files.glob("*/*/projects/*.yaml"):
            for d in ex._load_yaml(f):
                if d.get("kind") == "Project":
                    self.projects[d["metadata"]["name"]] = (d.get("spec") or {}).get("organizationRef") \
                        or self.installation
        for name in self.projects:
            if self.projects[name] is None:
                self.projects[name] = self.installation
        # A project is in the rehearsal when its repository is in the export (a slice holds a few).
        held = {d.name for d in files.glob("*/*") if d.is_dir()}
        self.projects = {p: o for p, o in self.projects.items() if p in held}

    def tenant_types(self, e: ex.Endpoint) -> dict:
        return self.index["tenants"].get(e.tenant, {})

    def granted(self, e: ex.Endpoint) -> dict:
        """{type: expected count or None} the public grants open, of the types the export holds."""
        out, _ = ex.expected_types(e, e.public_grants(), self.tenant_types(e))
        return out

    def temporal(self, e: ex.Endpoint, types) -> tuple[str, dict] | None:
        """The (type, slot) of the granted type with the newest exported instance."""
        per = self.index["temporal"].get(e.tenant, {})
        best = None
        for t in types:
            s = per.get(t)
            if s and (best is None or s["newest"] > best[1]["newest"]):
                best = (t, s)
        return best

    def klass(self, e: ex.Endpoint) -> str:
        types = self.granted(e) if not e.closed else ex.expected_types(e, e.grants, self.tenant_types(e))[0]
        tt = self.tenant_types(e)
        if not types:
            shape = "empty"
        elif self.temporal(e, types):
            shape = "history"
        elif any(tt.get(t, {}).get("location") for t in types):
            shape = "geo"
        else:
            shape = "rows"
        return f"{e.audience}/{shape}"

    def projects_of(self, org: str) -> list[str]:
        return sorted(p for p, o in self.projects.items() if o == org)


# ------------------------------------------------------------------------------------ suite

class Suite:
    def __init__(self, a, expect: Expect):
        self.a, self.x = a, expect
        self.http = Http(a.base_url, timeout=a.timeout)
        self.r = Results("mcp", a.base_url)
        now = dt.datetime.now(dt.timezone.utc)
        self.now = now
        self.window = now - dt.timedelta(hours=24)
        self.stale = now - dt.timedelta(hours=24, minutes=a.sweep_minutes)
        self.hub = Mcp(self.http, "/api/mcp")

    def check(self, *args, **kw):
        self.r.check(*args, **kw)

    # ------------------------------------------------------------ 1. hub, door level

    def hub_door(self, public: list[ex.Endpoint], closed: list[ex.Endpoint]):
        t = "hub /api/mcp"
        a = self.hub.initialize()
        info = (a.result or {}).get("serverInfo") if isinstance(a.result, dict) else None
        self.check("hub.initialize", t, "initialize", "200, serverInfo and protocolVersion",
                   a.brief(120), a.ok and bool(info) and bool((a.result or {}).get("protocolVersion")))
        a, tools = self.hub.tools()
        names = {x.get("name") for x in tools}
        missing = sorted(HUB_READS - names)
        self.check("hub.tools", t, "tools/list", f"lists {sorted(HUB_READS)}",
                   f"{len(names)} tools; missing {missing}", a.ok and not missing)
        writes = sorted(names & WRITE_TOOLS)
        self.check("hub.tools.no-write", t, "tools/list (anonymous)", "no write tool offered",
                   f"writes listed: {writes}", a.ok and not writes)
        # list_endpoints: every page, against the public Endpoints that serve mcp.
        listed, cursor, pages, last = set(), None, 0, None
        while pages < 200:
            args = {"limit": 50}
            if cursor:
                args["cursor"] = cursor
            last = self.hub.call("list_endpoints", args)
            pages += 1
            data = last.data
            rows = (data.get("endpoints") if isinstance(data, dict) else None) or entities_of(data)
            for row in rows or []:
                slug = row.get("slug") or row.get("endpoint") or row.get("id")
                if slug:
                    listed.add(str(slug))
            cursor = data.get("nextCursor") if isinstance(data, dict) else None
            if not last.ok or not cursor:
                break
        want = {e.slug for e in public if "mcp" in e.representations}
        lost, extra = sorted(want - listed), sorted(listed - want)
        self.check("hub.list_endpoints", t, "tools/call list_endpoints (every page)",
                   f"the {len(want)} public Endpoints that serve mcp", f"{len(listed)} listed in {pages} pages; "
                   f"missing {len(lost)} {lost[:5]}; extra {len(extra)} {extra[:5]}; last {last.brief(80)}",
                   bool(last and last.ok) and not lost and not extra)
        leaked = sorted(listed & {e.slug for e in closed})
        self.check("hub.list_endpoints.closed", t, "tools/call list_endpoints", "no closed Endpoint listed anonymously",
                   f"closed listed: {leaked[:5]}", not leaked)
        # SP-20: a closed Endpoint and one that does not exist are one answer.
        fake = "".join(secrets.choice("abcdefghijklmnopqrstuvwxyz234567") for _ in range(26))
        nope = self.hub.call("query_entities", {"endpoint": fake, "type": ["Thing"]})
        self.check("hub.unknown-endpoint", t, "tools/call query_entities endpoint=<random slug>", "refused",
                   nope.brief(120), nope.refused)
        if closed:
            c = closed[0]
            other = self.hub.call("query_entities", {"endpoint": c.slug, "type": ["Thing"]})
            same = text_of(other.result) == text_of(nope.result) and bool(other.error) == bool(nope.error)
            self.check("hub.closed-endpoint", f"{t} {c.project}/{c.name}",
                       f"tools/call query_entities endpoint={c.slug}",
                       "refused, the same answer as a slug that does not exist (SP-20)",
                       other.brief(120), other.refused and same)

    # ------------------------------------------------------------ 1. hub, per Endpoint

    def hub_endpoint(self, e: ex.Endpoint):
        t = f"hub {e.project}/{e.name} ({self.x.klass(e)})"
        types = self.x.granted(e)
        tt = self.x.tenant_types(e)
        if not types:
            self.r.skip("hub.query", t, "tools/call query_entities", "rows of the granted types",
                        f"the export holds no row of a type the public Policies of {e.slug} open")
            return
        typ = max(types, key=lambda k: tt[k]["count"])
        slot = tt[typ]
        a = self.hub.call("query_entities", {"endpoint": e.slug, "type": [typ], "limit": 5})
        got = entities_of(a.data)
        wrong = [x.get("id") for x in got if ex.short_type(str(x.get("type"))) != typ]
        self.check("hub.query.page", t, f"query_entities endpoint={e.slug} type={typ} limit=5",
                   f"1-5 {typ} entities (export holds {slot['count']})", f"{len(got)} entities; {a.brief(100)}",
                   a.ok and 0 < len(got) <= 5 and not wrong)
        sample = (slot.get("psample") or {}).get(next((s.pattern for s in e.selectors(e.public_grants()) or []
                                                       if s.type in (typ, "*") and s.pattern), None)) \
            or slot["sample"]
        a = self.hub.call("query_entities", {"endpoint": e.slug, "id": [sample], "type": [typ]})
        self.check("hub.query.by-id", t, f"query_entities endpoint={e.slug} id={sample}",
                   "the exported entity", a.brief(120), a.ok and sample in ids_in(a.data))
        want = types[typ]
        a = self.hub.call("query_entities", {"endpoint": e.slug, "type": [typ], "count": True, "limit": 1})
        total = total_of(a.data)
        if want is None:
            self.r.add("hub.query.count", t, f"query_entities type={typ} count=true", "a total",
                       f"total {total}", "pass" if a.ok and total is not None else "fail",
                       "the grants narrow the type (q, scopeQ, attributes or overlapping patterns): no export "
                       "count to compare")
        elif slot.get("expires"):
            self.check("hub.query.count", t, f"query_entities type={typ} count=true",
                       f"<= {want} (the type carries expiresAt)", f"total {total}",
                       a.ok and total is not None and 0 < total <= want)
        else:
            self.check("hub.query.count", t, f"query_entities type={typ} count=true", f"total {want} (export)",
                       f"total {total}; {a.brief(80) if total is None else ''}", a.ok and total == want)
        a = self.hub.call("describe_schema", {"endpoint": e.slug, "entityType": typ})
        self.check("hub.describe_schema", t, f"describe_schema endpoint={e.slug} entityType={typ}",
                   f"a schema naming {typ}", a.brief(100), a.ok and typ in json.dumps(a.result))
        self.hub_temporal(e, t, types)

    def hub_temporal(self, e, t, types, door=None, label="hub"):
        door = door or self.hub
        found = self.x.temporal(e, types)
        if not found:
            self.r.skip(f"{label}.temporal.24h", t, "query_temporal", "the exported instances",
                        "the export holds no instance of a granted type of this tenant in its 24 h window")
            return
        typ, s = found
        newest = ex.parse_time(s["newest"])
        args = {"type": [typ], "id": [s["id"]], "attrs": [s["attr"]], "timerel": "between",
                "timeAt": iso(self.window), "endTimeAt": iso(self.now + dt.timedelta(minutes=5))}
        if door is self.hub:
            args["endpoint"] = e.slug
        if newest < self.window:
            self.r.skip(f"{label}.temporal.24h", t, f"query_temporal id={s['id']}", "the exported instance",
                        f"its newest instance ({s['newest']}) aged out of the rolling 24 h window")
        else:
            a = door.call("query_temporal", args)
            n = instances(a.data)
            self.check(f"{label}.temporal.24h", t,
                       f"query_temporal id={s['id']} attrs={s['attr']} timerel=between {args['timeAt']}..now",
                       f">= 1 instance (newest exported {s['newest']})", f"{n} instances; {a.brief(80)}",
                       a.ok and n >= 1 and s["id"] in json.dumps(a.data))
        old = dict(args, timeAt=iso(self.stale - dt.timedelta(days=30)), endTimeAt=iso(self.stale))
        a = door.call("query_temporal", old)
        n = instances(a.data)
        self.check(f"{label}.temporal.older", t, f"query_temporal id={s['id']} between {old['timeAt']}..{old['endTimeAt']}",
                   "no instance older than 24 h (the rehearsal keeps one day)", f"{n} instances; {a.brief(80)}",
                   a.ok and n == 0)

    def hub_write(self, e: ex.Endpoint, door: Mcp | None = None, label: str = "hub"):
        door = door or self.hub
        t = f"{label} {e.project}/{e.name}"
        probe = f"urn:ngsi-ld:{SMOKE_TYPE}:{secrets.token_hex(6)}"
        args = {"entity": {"id": probe, "type": SMOKE_TYPE, "note": {"type": "Property", "value": "smoke"}}}
        if door is self.hub:
            args["endpoint"] = e.slug
        a = door.call("create_entity", args)
        self.check(f"{label}.write.refused", t, "tools/call create_entity (anonymous)", "refused", a.brief(120),
                   a.refused)
        q = {"id": [probe], "type": [SMOKE_TYPE]}
        if door is self.hub:
            q["endpoint"] = e.slug
        b = door.call("query_entities", q)
        self.check(f"{label}.write.nothing", t, f"query_entities id={probe}", "the refused entity is not there",
                   b.brief(100), probe not in ids_in(b.data))

    def hub_search(self, e: ex.Endpoint):
        types = self.x.granted(e)
        if not types:
            return
        typ = max(types, key=lambda k: self.x.tenant_types(e)[k]["count"])
        t = f"hub {e.project}/{e.name} ({self.x.klass(e)})"
        a = self.hub.call("search", {"query": typ})
        text = json.dumps(a.result or {})
        self.check("hub.search", t, f"search query={typ}", f"hits, one naming an Endpoint that holds {typ}",
                   a.brief(120), a.ok and any(x.slug in text for x in self.by_type.get(typ, [])))

    # ------------------------------------------------------------ 2. an Endpoint's own MCP

    def endpoint_door(self, e: ex.Endpoint):
        t = f"endpoint-mcp {e.project}/{e.name} ({self.x.klass(e)})"
        door = Mcp(self.http, f"/api/endpoint/{e.slug}/mcp")
        if e.closed:
            a = door.initialize()
            self.check("endpoint.closed.401", t, f"POST /api/endpoint/{e.slug}/mcp initialize (anonymous)",
                       "401 with WWW-Authenticate", f"{a.status} {a.headers.get('www-authenticate', '')[:80]}",
                       a.status == 401 and "www-authenticate" in a.headers)
            return
        a = door.initialize()
        self.check("endpoint.initialize", t, f"POST /api/endpoint/{e.slug}/mcp initialize", "200 with serverInfo",
                   a.brief(100), a.ok and isinstance(a.result, dict) and bool(a.result.get("serverInfo")))
        a, tools = door.tools()
        names = {x.get("name") for x in tools}
        self.check("endpoint.tools", t, "tools/list (anonymous)", "query_entities listed, no write tool",
                   f"{len(names)} tools; writes {sorted(names & WRITE_TOOLS)}",
                   a.ok and "query_entities" in names and not names & WRITE_TOOLS)
        types = self.x.granted(e)
        if types:
            tt = self.x.tenant_types(e)
            typ = max(types, key=lambda k: tt[k]["count"])
            sample = tt[typ]["sample"]
            a = door.call("query_entities", {"type": [typ], "id": [sample]})
            self.check("endpoint.query.by-id", t, f"query_entities type={typ} id={sample}", "the exported entity",
                       a.brief(120), a.ok and sample in ids_in(a.data))
            self.hub_temporal(e, t, types, door=door, label="endpoint")
        else:
            self.r.skip("endpoint.query.by-id", t, "query_entities", "the exported entity",
                        "the export holds no row of a type the public Policies open")
        self.hub_write(e, door=door, label="endpoint")

    # ------------------------------------------------------------ 3. Portal MCP per persona

    def portal_door(self):
        t = "portal /api/v1/mcp"
        a = Mcp(self.http, "/api/v1/mcp", retries=0).initialize()
        challenge = a.headers.get("www-authenticate", "")
        self.check("portal.no-token", t, "POST /api/v1/mcp initialize without a token",
                   "401 naming the resource metadata", f"{a.status} {challenge[:120]}",
                   a.status == 401 and "resource_metadata" in challenge)
        a = Mcp(self.http, "/api/v1/mcp", token="not.a.token", retries=0).initialize()
        self.check("portal.bad-token", t, "POST /api/v1/mcp initialize with a forged token", "401", f"{a.status}",
                   a.status == 401)

    def plan(self):
        """Which projects the personas are bound in, from the export."""
        x = self.x
        inst = x.installation
        if not inst:
            return None, "the export names no installation Organization"

        def score(project):
            return sum(1 for e in x.endpoints if e.project == project and x.granted(e))

        own = self.a.project or max(x.projects_of(inst), key=score, default=None)
        others = [(o, max(x.projects_of(o), key=score)) for o in sorted(x.orgs) if o != inst and x.projects_of(o)]
        if not own:
            return None, f"the installation Organization {inst} owns no project in the export"
        other_org, foreign = (others[0] if others else (None, None))
        return {"inst": inst, "own": own, "other_org": other_org, "foreign": foreign}, ""

    def personas(self, fx, plan):
        from personas import wait_until
        roles = {"platform": ("org-admin", {"organization": plan["inst"]}),
                 "project": ("steward", {"project": plan["own"]})}
        if plan["other_org"]:
            roles["organization"] = ("org-admin", {"organization": plan["other_org"]})
        fx.setup(roles)
        doors = {k: Mcp(self.http, "/api/v1/mcp", token=p.token) for k, p in fx.people.items()}

        def listed(k, project):
            a, tools = doors[k].tools({"project": project})
            return a, {x.get("name") for x in tools}

        # The reconciler reads the bindings on its next sync.
        wait_until(lambda: ADMIN_TOOLS <= listed("project", plan["own"])[1], self.a.bind_wait)
        own, foreign, inst = plan["own"], plan["foreign"], plan["inst"]
        cases = [("platform", ORG_NAMESPACE, True), ("platform", own, True), ("project", own, True),
                 ("project", ORG_NAMESPACE, False)]
        if foreign:
            cases += [("platform", foreign, False), ("organization", foreign, True), ("organization", own, False),
                      ("project", foreign, False)]
        for k, project, offered in cases:
            a, names = listed(k, project)
            got = sorted(names & ADMIN_TOOLS)
            self.check(f"portal.{k}.tools", f"portal {k} in {project}", f"tools/list project={project}",
                       f"administration tools {'all' if offered else 'none'}", f"{len(names)} tools; admin {got}",
                       a.ok and (set(got) == ADMIN_TOOLS if offered else not got))
        # The project list each reads.
        mine = {"platform": (ORG_NAMESPACE, set(self.x.projects_of(inst)), {foreign} - {None}),
                "project": (own, {own}, set(self.x.projects) - {own})}
        if foreign:
            mine["organization"] = (foreign, set(self.x.projects_of(plan["other_org"])),
                                    set(self.x.projects_of(inst)))
        for k, (where, want, never) in mine.items():
            a = doors[k].call("jc_organization_read", {"project": where, "aspect": "projects"})
            seen = {i.get("name") for i in (a.data or {}).get("items", [])} if isinstance(a.data, dict) else set()
            lost, leak = sorted(want - seen), sorted(seen & never)
            self.check(f"portal.{k}.projects", f"portal {k}", f"jc_organization_read project={where} aspect=projects",
                       f"{sorted(want)[:8]} and nothing else", f"{len(seen)} seen; missing {lost[:5]}; foreign {leak[:5]}; "
                       f"{'' if a.ok else a.brief(100)}", a.ok and not lost and not leak)
        if foreign:
            for k in ("project", "organization" if foreign else None):
                where = foreign if k == "project" else own
                a = doors[k].call("jc_organization_read", {"project": where, "aspect": "projects"})
                self.check(f"portal.{k}.projects.foreign", f"portal {k}",
                           f"jc_organization_read project={where} aspect=projects", "refused (404, PF-59)",
                           a.brief(120), a.refused)
        # Installation readings: the platform administrator's alone.
        for aspect in ("setup", "health", "brokers"):
            a = doors["platform"].call("jc_organization_read", {"project": ORG_NAMESPACE, "aspect": aspect})
            self.check(f"portal.platform.{aspect}", "portal platform",
                       f"jc_organization_read project=org aspect={aspect}", "answered", a.brief(100), a.ok)
            for k, where in (("project", own), ("organization", foreign)):
                if k not in doors:
                    continue
                a = doors[k].call("jc_organization_read", {"project": where, "aspect": aspect})
                self.check(f"portal.{k}.{aspect}.refused", f"portal {k}",
                           f"jc_organization_read project={where} aspect={aspect}", "refused (PF-03)",
                           a.brief(100), a.refused)
        # A manifest of the export by name, and a data query through its Endpoint.
        for k, project in (("project", own), ("platform", own), ("organization", foreign)):
            if k not in doors or not project:
                continue
            e = next((e for e in self.x.endpoints if e.project == project and self.x.granted(e)), None)
            if not e:
                continue
            a = doors[k].call("jc_resource_get", {"project": project, "kind": "Endpoint", "name": e.name})
            self.check(f"portal.{k}.resource_get", f"portal {k} in {project}",
                       f"jc_resource_get kind=Endpoint name={e.name}", f"the manifest with slug {e.slug}",
                       a.brief(100), a.ok and e.slug in json.dumps(a.result))
            self.portal_query(doors[k], f"portal.{k}.data_query", f"portal {k} in {project}", project, e)
            if k == "project" and foreign:
                f = next((f for f in self.x.endpoints if f.project == foreign), None)
                if f:
                    a = doors[k].call("jc_resource_get", {"project": foreign, "kind": "Endpoint", "name": f.name})
                    self.check("portal.project.resource_get.foreign", f"portal project in {foreign}",
                               f"jc_resource_get kind=Endpoint name={f.name}", "refused (404)", a.brief(100),
                               a.refused and f.slug not in json.dumps(a.result or {}))
        # Writes each is refused: a new project (O, A), another Organization's settings (O, A),
        # a space in a project it does not hold (A, O).
        for k in ("organization", "project"):
            if k not in doors:
                continue
            a = doors[k].call("jc_project_create", {"project": ORG_NAMESPACE if k == "organization" else own,
                                                    "name": f"smoke-mcp-{secrets.token_hex(3)}"})
            self.check(f"portal.{k}.project_create.refused", f"portal {k}", "jc_project_create",
                       "refused (ADR-N-058 §2.2, PF-65)", a.brief(120), a.refused)
            org = {"apiVersion": "joinedcontext.com/v1alpha1", "kind": "Organization",
                   "metadata": {"name": inst, "namespace": ORG_NAMESPACE}, "spec": self.x.orgs[inst]}
            a = doors[k].call("jc_resource_propose", {"project": ORG_NAMESPACE, "manifest": org})
            self.check(f"portal.{k}.propose_installation.refused", f"portal {k}",
                       f"jc_resource_propose kind=Organization name={inst}", "refused", a.brief(120), a.refused)
            elsewhere = foreign if k == "project" else own
            if elsewhere:
                space = {"apiVersion": "joinedcontext.com/v1alpha1", "kind": "ContextSpace",
                         "metadata": {"name": "smoke-mcp", "namespace": elsewhere}, "spec": {}}
                a = doors[k].call("jc_resource_propose", {"project": elsewhere, "manifest": space})
                self.check(f"portal.{k}.propose_elsewhere.refused", f"portal {k} in {elsewhere}",
                           "jc_resource_propose kind=ContextSpace", "refused (403 or 404)", a.brief(120), a.refused)
        # People of the realm: the platform administrator's (P), refused to the others.
        a = doors["platform"].call("jc_person_list", {"project": ORG_NAMESPACE})
        names = json.dumps(a.result or {})
        self.check("portal.platform.person_list", "portal platform", "jc_person_list",
                   "the persons of the realm, the smoke personas among them", a.brief(100),
                   a.ok and fx.people["project"].username in names)
        for k, where in (("project", own), ("organization", foreign)):
            if k in doors:
                a = doors[k].call("jc_person_list", {"project": where})
                self.check(f"portal.{k}.person_list.refused", f"portal {k}", "jc_person_list", "refused",
                           a.brief(100), a.refused)
        return doors

    def portal_query(self, door, id, target, project, e, args_extra=None, expect=True):
        types = self.x.granted(e) or ex.expected_types(e, e.grants, self.x.tenant_types(e))[0]
        if not types:
            self.r.skip(id, target, "jc_data_query", "the exported entity", "no exported row of a granted type")
            return None
        tt = self.x.tenant_types(e)
        typ = max(types, key=lambda k: tt[k]["count"])
        sample = tt[typ]["sample"]
        args = {"project": project, "endpoint": e.name, "type": typ, "id": sample, **(args_extra or {})}
        a = door.call("jc_data_query", args)
        if expect:
            self.check(id, target, f"jc_data_query endpoint={e.name} type={typ} id={sample}", "the exported entity",
                       a.brief(120), a.ok and sample in ids_in(a.data))
        else:
            self.check(id, target, f"jc_data_query endpoint={e.name} id={sample}", "refused, no data",
                       a.brief(120), a.refused and sample not in json.dumps(a.result or {}))
        return a

    # ------------------------------------------------------------ 4. a connector

    def connector(self, fx, plan):
        owner = fx.people["project"]
        own = plan["own"]
        eps = [e for e in self.x.endpoints if e.project == own and self.x.granted(e)]
        if not eps:
            self.r.skip("connector", f"connector in {own}", "POST /api/v1/connectors", "a connector over one Endpoint",
                        f"no Endpoint of {own} with exported rows")
            return
        e, outside = eps[0], (eps[1] if len(eps) > 1 else None)
        body = {"name": f"smoke-mcp-{secrets.token_hex(3)}", "segments": ["data-read"],
                "scope": [{"project": own, "endpoint": e.name, "access": "read"}],
                "expiresAt": iso(self.now + dt.timedelta(days=1))}
        st, made = fx.create_connector(owner, body)
        t = f"connector {own}/{e.name}"
        self.check("connector.create", t, "POST /api/v1/connectors (the project administrator's session)",
                   "201 active", f"{st} {str(made)[:160]}", st == 201 and isinstance(made, dict)
                   and made.get("status", made.get("state", "active")) == "active")
        if st != 201 or not isinstance(made, dict):
            return
        cid = made["id"]
        path = f"/c/{cid}"
        a = Mcp(self.http, path, retries=0).initialize()
        self.check("connector.no-token", t, f"POST {path} initialize without a token",
                   "401 naming scope connector:<id>", f"{a.status} {a.headers.get('www-authenticate', '')[:120]}",
                   a.status == 401 and f"connector:{cid}" in a.headers.get("www-authenticate", ""))
        a = Mcp(self.http, path, token=owner.token, retries=0).initialize()
        self.check("connector.portal-token", t, f"POST {path} with the owner's mcp:portal token",
                   "401 (not a token for this connector)", f"{a.status}", a.status == 401)
        try:
            token = fx.mcp_token(owner, scope=f"connector:{cid}")
        except Exception as err:  # noqa: BLE001 - every failure is a row
            self.check("connector.token", t, f"mcp-local code flow scope=connector:{cid}", "a token", str(err)[:200],
                       False)
            return
        door = Mcp(self.http, path, token=token)
        a = door.initialize()
        self.check("connector.initialize", t, f"POST {path} initialize", "200", a.brief(100), a.ok)
        a, tools = door.tools()
        names = {x.get("name") for x in tools}
        self.check("connector.tools", t, "tools/list", "jc_data_query listed, no administration tool",
                   f"{len(names)} tools; admin {sorted(names & ADMIN_TOOLS)}",
                   a.ok and "jc_data_query" in names and not names & ADMIN_TOOLS)
        self.portal_query(door, "connector.data_query", t, own, e)
        if outside:
            self.portal_query(door, "connector.outside-scope", f"{t} -> {outside.name}", own, outside, expect=False)
        a = door.call("jc_organization_read", {"project": own, "aspect": "projects"})
        self.check("connector.admin.refused", t, "jc_organization_read", "refused (outside the segments)",
                   a.brief(100), a.refused)
        st = fx.revoke_connector(cid)
        a = door.initialize()
        self.check("connector.revoked", t, f"DELETE /api/v1/connectors/{cid}, then POST {path}",
                   "the revoke answers 2xx, the connector answers nothing after it", f"revoke {st}; then {a.brief(80)}",
                   200 <= st < 300 and (a.status in (401, 403, 404) or a.refused))

    # ------------------------------------------------------------ run

    def run(self):
        x = self.x
        public = [e for e in x.endpoints if e.audience == "public"]
        closed = [e for e in x.endpoints if e.closed]
        self.by_type = defaultdict(list)
        for e in public:
            for t in x.granted(e):
                self.by_type[t].append(e)
        self.hub_door(public, closed)
        served = [e for e in public if "mcp" in e.representations]
        classes = defaultdict(list)
        for e in served + [c for c in closed if "mcp" in c.representations]:
            classes[x.klass(e)].append(e)
        chosen = served if not self.a.per_class else \
            [e for k, v in classes.items() for e in v[:self.a.per_class] if not e.closed]
        with cf.ThreadPoolExecutor(self.a.workers) as pool:
            list(pool.map(self.hub_endpoint, chosen))
            firsts = [v[0] for v in classes.values()]
            list(pool.map(self.endpoint_door, firsts))
            list(pool.map(self.hub_search, [v[0] for k, v in classes.items() if not v[0].closed]))
            list(pool.map(self.hub_write, [v[0] for k, v in classes.items() if not v[0].closed]))
        self.portal_door()
        plan, why = self.plan()
        if not self.a.state:
            why = "no --state: the personas need the rehearsal's realm admin secret and shako token"
        if why:
            for id in ("portal.personas", "connector"):
                self.r.skip(id, "portal /api/v1/mcp", "personas", "the MCP-ADMIN.md matrix", why)
            return {"classes": {k: len(v) for k, v in classes.items()}}
        from personas import Fixture
        fx = Fixture(self.a.base_url, Path(self.a.state), self.a.config_repo)
        try:
            try:
                self.personas(fx, plan)
            except Exception as err:  # noqa: BLE001 - a broken fixture is a failing row, never a pass
                self.check("portal.personas.setup", "portal /api/v1/mcp", "persons, RoleBindings, mcp-local tokens",
                           "three administrators with tokens", str(err)[:240], False)
                return {"plan": plan}
            try:
                self.connector(fx, plan)
            except Exception as err:  # noqa: BLE001
                self.check("connector.setup", "connector", "POST /api/v1/connectors", "a connector",
                           str(err)[:240], False)
        finally:
            if not self.a.keep:
                left = fx.cleanup()
                if left:
                    self.check("cleanup", "rehearsal", "remove smoke-mcp-* people, bindings, connectors",
                               "all removed", "; ".join(left)[:240], False)
        return {"plan": plan, "classes": {k: len(v) for k, v in classes.items()},
                "personas": {k: p.username for k, p in fx.people.items()}}


def main(argv=None):
    p = argparse.ArgumentParser(prog="run mcp", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--export", required=True, help="the export bundle the rehearsal was built from")
    p.add_argument("--out", required=True)
    p.add_argument("--state", help="the rehearsal's state dir ($REHEARSAL_HOME/<env>/state)")
    p.add_argument("--config-repo", default="joinedcontext/configuration")
    p.add_argument("--project", help="the project the project administrator is bound in (default: the "
                                     "installation Organization's project with most Endpoints holding rows)")
    p.add_argument("--per-class", type=int, default=0,
                   help="hub data checks on at most N Endpoints per class (default 0: every public Endpoint)")
    p.add_argument("--workers", type=int, default=6)
    p.add_argument("--timeout", type=float, default=60)
    p.add_argument("--sweep-minutes", type=int, default=70,
                   help="the rehearsal's retain interval plus slack: instances up to 24 h + this are not a leak")
    p.add_argument("--bind-wait", type=float, default=120, help="seconds to wait for the reconciler to read the bindings")
    p.add_argument("--cache", default=str(Path.home() / ".cache" / "jc-smoke"))
    p.add_argument("--keep", action="store_true", help="leave the smoke-mcp-* people, bindings and connectors")
    p.add_argument("--admin-token-file", help="unused by this suite (accepted for the common runner)")
    p.add_argument("--person-token-file", help="unused by this suite: it makes its own personas")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    read_token(a.admin_token_file)
    x = Expect(Path(a.export), Path(a.cache))
    s = Suite(a, x)
    meta = s.run()
    meta = {"export": a.export, "exportedAt": x.index.get("exportedAt"), **(meta or {})}
    s.r.write(Path(a.out), {k: json.dumps(v) if isinstance(v, (dict, list)) else v for k, v in meta.items()})
    t = s.r.totals()
    print(f"mcp: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip -> {a.out}/REPORT.md")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
