"""Smoke suite `multiorg`: one jc serving several Organizations, each isolated from the others.

    bins/jc/smoke/run multiorg --base-url http://localhost:42301 --state <rehearsal state> \
        --export <bundle composed by compose.py> --out <dir>

Expectations come from the bundle (manifest.json `multiorg`, the project repositories' manifests).
--state is the rehearsal's state dir (rehearsal.py): the realm admin client and the forge token
there make the test persons (`mo-*`, random passwords kept in memory) and their RoleBindings.

Personas: `mo-platform` (group platform-admins) and, per Organization O, `mo-O-admin` (RoleBinding
role org-admin, scope organization O) and `mo-O-member` (role viewer, scope O). Each foreign read is
paired with a control: the same call by the owner Organization's admin must answer 200, so a 404
that means "no such route" never passes for isolation."""
import argparse
import base64
import hashlib
import json
import re
import secrets
import sys
import time
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "deploy" / "rehearsal"))
import rehearsal as R  # noqa: E402  (Http, sign-in, person helpers of the rehearsal kit)

CONFIG = ("joinedcontext", "configuration")
DENIED = (403, 404)
MCP_CLIENT = "mcp-local"  # the realm's public client for a local MCP client
# Project-scoped reads every Organization's people use; each is checked both ways.
PROJECT_READS = ["", "/spaces", "/endpoints", "/pipelines", "/apps", "/datamodels", "/datasources",
                 "/changes", "/dashboards", "/policies", "/rolebindings", "/ckaninstances",
                 "/serviceaccounts", "/activity", "/syncsources", "/mqttbrokers", "/drafts"]


class Suite:
    def __init__(self, a):
        self.a = a
        self.rows = []
        self.base = a.base_url.rstrip("/")
        self.state = Path(a.state) if a.state else None
        self.export = Path(a.export)
        self.people: dict[str, dict] = {}

    # ---- results
    def row(self, id_, target, call, expected, actual, ok, reason=""):
        res = "skip" if ok is None else ("pass" if ok else "fail")
        self.rows.append({"id": id_, "target": target, "call": call, "expected": expected,
                          "actual": actual, "result": res, "reason": reason})
        return ok

    # ---- expectations from the bundle
    def expectations(self):
        m = json.loads((self.export / "manifest.json").read_text())["multiorg"]["organizations"]
        files = self.export / "gitea" / "files"
        exp = {}
        for org, v in m.items():
            exp[org] = {"projects": {}}
            for p in v["projects"]:
                kinds = {"Endpoint": [], "Pipeline": [], "CkanInstance": []}
                for f in (files / CONFIG[0] / p).rglob("*.yaml"):
                    text = f.read_text(errors="replace")
                    for kind in kinds:
                        if re.search(rf"^kind:\s*{kind}\s*$", text, re.M):
                            n = re.search(r"^metadata:\s*\n(?:\s+.*\n)*?\s+name:\s*([\w.-]+)", text, re.M)
                            if n:
                                kinds[kind].append(n.group(1))
                ckan = []
                for f in (files / CONFIG[0] / p / "ckan").glob("*.yaml"):
                    d = re.search(r"organizationDefault:\s*([\w.-]+)", f.read_text())
                    if d:
                        ckan.append(d.group(1))
                exp[org]["projects"][p] = {k: sorted(set(v)) for k, v in kinds.items()} | {"ckanOrgs": ckan}
        self.exp = exp
        orgs = files / CONFIG[0] / CONFIG[1] / "organizations"
        self.installation = next((f.stem for f in orgs.glob("*.yaml")
                                  if re.search(r"^\s*installation:\s*true", f.read_text(), re.M)), None)
        return exp

    # ---- personas
    def client(self) -> R.Http:
        return R.Http(self.base)

    def setup(self):
        h = self.client()
        tok = R.admin_token(h, self.state)
        names = {"platform": ["/platform-admins"]}
        for org in self.exp:
            names[f"{org}-admin"] = []
            names[f"{org}-member"] = []
        for key, groups in names.items():
            uid, pw = R.make_person(h, tok, "jc", f"mo-{key}", groups=groups)
            self.people[key] = {"username": f"mo-{key}", "id": uid, "password": pw}
        # Their RoleBindings, in the installation's configuration repository (a forge commit the
        # Portal syncs, as a person's change would be once approved).
        files = []
        for org in self.exp:
            for kind, role in (("admin", "org-admin"), ("member", "viewer")):
                p = self.people[f"{org}-{kind}"]
                files.append((f"users/assignments/multiorg-{org}-{kind}.yaml",
                              "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n"
                              f"  name: multiorg-{org}-{kind}\n  namespace: org\nspec:\n  subjects:\n"
                              f"    - user: {p['id']} # {p['username']}\n  role: {role}\n"
                              f"  scope: {{ organization: {org} }}\n"))
        self.commit(files, "multiorg smoke: test persons' RoleBindings")
        # Wait until each admin sees its Organization's projects (the Portal's sync).
        deadline = time.time() + self.a.sync_timeout
        pending = set(self.exp)
        while pending and time.time() < deadline:
            for org in sorted(pending):
                st, body = self.session(f"{org}-admin").json("GET", "/api/v1/projects")
                if st == 200 and set(self.exp[org]["projects"]) <= {i["name"] for i in body.get("items", [])}:
                    pending.discard(org)
            if pending:
                time.sleep(5)
        self.row("setup.bindings-synced", ",".join(self.exp), "GET /api/v1/projects (each org admin)",
                 "every admin lists its projects", f"not yet: {sorted(pending)}" if pending else "all",
                 not pending)

    def commit(self, files, message):
        h = self.client()
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        owner, repo = CONFIG
        ops = []
        for path, text in files:
            st, cur = h.json("GET", f"/git/api/v1/repos/{owner}/{repo}/contents/{path}?ref=main", headers=auth)
            op = {"operation": "update" if st == 200 else "create", "path": path,
                  "content": base64.b64encode(text.encode()).decode()}
            if st == 200:
                if base64.b64decode(cur.get("content", "")).decode() == text:
                    continue
                op["sha"] = cur["sha"]
            ops.append(op)
        if ops:
            st, body = h.json("POST", f"/git/api/v1/projects/{owner}/repositories/{repo}/contents", headers=auth,
                              body={"branch": "main", "files": ops, "message": message})
            if st not in (200, 201):
                raise RuntimeError(f"commit {message}: {st} {body}")

    def session(self, key) -> R.Http:
        p = self.people[key]
        if "http" not in p:
            h = self.client()
            ok, detail = R.sign_in(h, p["username"], p["password"])
            if not ok:
                raise RuntimeError(f"sign-in of {p['username']}: {detail}")
            p["http"] = h
        return p["http"]

    def bearer(self, key) -> str | None:
        """An MCP access token of the person, as a local MCP client gets one: code flow with PKCE
        through the realm's public client mcp-local, scope mcp:portal (audience portal-api); the
        redirect to the loopback callback is read, never followed."""
        p = self.people[key]
        if "token" in p:
            return p["token"]
        h = self.client()
        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        redirect = "http://127.0.0.1/callback"
        q = urllib.parse.urlencode({"client_id": MCP_CLIENT, "redirect_uri": redirect, "response_type": "code",
                                    "scope": "openid mcp:portal", "state": "mo", "code_challenge": challenge,
                                    "code_challenge_method": "S256"})
        login_url, st, page = R.follow(h, f"/id/realms/jc/protocol/openid-connect/auth?{q}")
        p["token"] = None
        if st != 200:
            return None
        text = page.decode(errors="replace")
        form = {"csrf_token": R._hidden(text, "csrf_token"), "session_code": R._hidden(text, "session_code"),
                "username": p["username"], "password": p["password"]}
        st, hdrs, _ = h.call("POST", login_url, form=form)
        loc = hdrs.get("Location") or hdrs.get("location") or ""
        if "code=" not in loc:
            return None
        code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query)["code"][0]
        st, tok = h.json("POST", "/id/realms/jc/protocol/openid-connect/token", form={
            "grant_type": "authorization_code", "client_id": MCP_CLIENT, "code": code,
            "redirect_uri": redirect, "code_verifier": verifier})
        p["token"] = tok.get("access_token") if st == 200 and isinstance(tok, dict) else None
        return p["token"]

    # ---- checks
    def check_projects(self):
        everything = {p for o in self.exp.values() for p in o["projects"]}
        st, body = self.session("platform").json("GET", "/api/v1/projects")
        seen = {i["name"] for i in (body or {}).get("items", [])} if st == 200 else set()
        self.row("projects.platform-sees-all", "platform", "GET /api/v1/projects",
                 f"{len(everything)} projects of {len(self.exp)} orgs", f"{st}, missing {sorted(everything - seen)}",
                 st == 200 and everything <= seen)
        for org, e in self.exp.items():
            for kind in ("admin", "member"):
                st, body = self.session(f"{org}-{kind}").json("GET", "/api/v1/projects")
                items = (body or {}).get("items", []) if st == 200 else []
                seen = {i["name"] for i in items}
                foreign = sorted(seen & (everything - set(e["projects"])))
                wrong_org = sorted(i["name"] for i in items if i.get("organization") not in (org, None))
                self.row(f"projects.{org}-{kind}.own", org, "GET /api/v1/projects",
                         f"all of {sorted(e['projects'])}", f"{st}, missing {sorted(set(e['projects']) - seen)}",
                         st == 200 and set(e["projects"]) <= seen)
                self.row(f"projects.{org}-{kind}.no-foreign", org, "GET /api/v1/projects",
                         "no project of another org", f"foreign {foreign} {wrong_org}", st == 200 and not foreign and not wrong_org)

    def check_project_reads(self):
        """Every project read both ways: A's admin and member get 403/404 on B, B's admin 200."""
        for owner, e in self.exp.items():
            for project, objs in e["projects"].items():
                calls = [f"/api/v1/projects/{project}{suffix}" for suffix in PROJECT_READS]
                calls += [f"/api/v1/projects/{project}/endpoints/{n}" for n in objs["Endpoint"][:2]]
                calls += [f"/api/v1/projects/{project}/pipelines/{n}" for n in objs["Pipeline"][:2]]
                for call in calls:
                    st_own, _, _ = self.session(f"{owner}-admin").call("GET", call)
                    for other in self.exp:
                        if other == owner:
                            continue
                        for kind in ("admin", "member"):
                            st, _, body = self.session(f"{other}-{kind}").call("GET", call)
                            rid = f"isolation.{other}-{kind}.{project}{call.split(project, 1)[1] or '/'}"
                            if st_own != 200:
                                self.row(rid, f"{other}->{owner}", f"GET {call}", "control 200, foreign 403/404",
                                         f"control {st_own}, foreign {st}", st in DENIED and None,
                                         f"no control: {owner}-admin got {st_own}" if st in DENIED
                                         else f"foreign got {st}, control {st_own}")
                                if st not in DENIED:
                                    self.rows[-1]["result"] = "fail"
                                continue
                            self.row(rid, f"{other}->{owner}", f"GET {call}", "403/404 (control 200)",
                                     f"{st} {body[:120]!r}" if st not in DENIED else str(st), st in DENIED)

    def check_writes(self):
        """A foreign org admin's DELETE of an Endpoint is refused and the Endpoint stays."""
        for owner, e in self.exp.items():
            project = next((p for p, o in e["projects"].items() if o["Endpoint"]), None)
            if not project:
                continue
            ep = e["projects"][project]["Endpoint"][0]
            call = f"/api/v1/projects/{project}/endpoints/{ep}"
            for other in self.exp:
                if other == owner:
                    continue
                st, _, body = self.session(f"{other}-admin").call("DELETE", call)
                st_after, _, _ = self.session(f"{owner}-admin").call("GET", call)
                self.row(f"write.{other}-admin.delete.{project}/{ep}", f"{other}->{owner}", f"DELETE {call}",
                         "403/404, still there", f"{st}, owner then reads {st_after}",
                         st in DENIED and st_after == 200)

    def check_people(self):
        """The members list of an org admin names nobody bound only to another Organization."""
        for org in self.exp:
            st, body = self.session(f"{org}-admin").json("GET", "/api/v1/organization/people")
            emails = {i.get("email", "") for i in (body or {}).get("items", [])} if st == 200 else set()
            own = {f"mo-{org}-admin@rehearsal.invalid", f"mo-{org}-member@rehearsal.invalid"}
            foreign = sorted(x for x in emails if re.match(r"mo-(.+)-(admin|member)@", x)
                             and re.match(r"mo-(.+)-(admin|member)@", x).group(1) != org)
            if st in DENIED:
                self.row(f"people.{org}-admin", org, "GET /api/v1/organization/people",
                         "own members, nobody of another org", f"{st}", None,
                         "an org admin may not list people at all here")
                continue
            self.row(f"people.{org}-admin.own", org, "GET /api/v1/organization/people", f"{sorted(own)} listed",
                     f"{st}, missing {sorted(own - emails)}", st == 200 and own <= emails)
            self.row(f"people.{org}-admin.no-foreign", org, "GET /api/v1/organization/people",
                     "nobody of another org", f"foreign {foreign}", st == 200 and not foreign)

    def check_mcp(self):
        everything = {p for o in self.exp.values() for p in o["projects"]}
        for org, e in self.exp.items():
            for kind in ("admin", "member"):
                key = f"{org}-{kind}"
                tok = self.bearer(key)
                if not tok:
                    self.row(f"mcp.{key}", org, "POST /api/v1/mcp", "a token", "none", None,
                             "no bearer token for the person (code flow through client mcp-local failed)")
                    continue
                h = self.client()
                auth = {"Authorization": f"Bearer {tok}", "Accept": "application/json, text/event-stream"}
                st, init = h.json("POST", "/api/v1/mcp", headers=auth, body={
                    "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "multiorg-smoke", "version": "1"}}})
                text = json.dumps(init)
                foreign = sorted(p for p in everything - set(e["projects"]) if re.search(rf"\b{re.escape(p)}\b", text))
                self.row(f"mcp.{key}.initialize", org, "POST /api/v1/mcp initialize",
                         "200, instructions name no foreign project", f"{st}, foreign {foreign}",
                         st == 200 and not foreign)
                st, lst = h.json("POST", "/api/v1/mcp", headers=auth,
                                 body={"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
                tools = [t["name"] for t in ((lst or {}).get("result") or {}).get("tools", [])] if st == 200 else []
                self.people[key]["tools"] = sorted(tools)
                self.row(f"mcp.{key}.tools-list", org, "POST /api/v1/mcp tools/list", "200 with tools",
                         f"{st}, {len(tools)} tools", st == 200 and bool(tools))
                self.mcp_calls(key, org, h, auth)

    def foreign_marks(self, org):
        """Text that names another Organization's objects: its persons' usernames, its projects
        quoted as JSON strings, and its Endpoint names unique to it (none an own name)."""
        own = {n for p, o in self.exp[org]["projects"].items() for n in [p, *o["Endpoint"], *o["Pipeline"]]}
        marks = {}
        for other, oe in self.exp.items():
            if other == org:
                continue
            marks[f"mo-{other}-admin"] = other
            marks[f"mo-{other}-member"] = other
            for p, o in oe["projects"].items():
                if p not in own:
                    marks[f'"{p}"'] = other
                for n in o["Endpoint"]:
                    if n not in own and len(n) >= 8:
                        marks[f'"{n}"'] = other
        return marks

    def mcp_calls(self, key, org, h, auth):
        """Org-wide MCP reads name nothing of another Organization; per-project reads of a foreign
        project answer an error naming none of its objects."""
        marks = self.foreign_marks(org)
        installation_admin = org == self.installation and key.endswith("-admin")

        def call(name, args):
            st, res = h.json("POST", "/api/v1/mcp", headers=auth, body={
                "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": name, "arguments": args}})
            return st, res

        def leaks(text, skip=()):
            return sorted({m for m in marks if m in text and m.strip('"') not in skip})

        own = next(p for p, o in sorted(self.exp[org]["projects"].items()) if o["Endpoint"])
        st, res = call("jc_resource_list", {"project": own, "kind": "Endpoint"})
        text = json.dumps(res, ensure_ascii=False)
        missing = [n for n in self.exp[org]["projects"][own]["Endpoint"] if f'\\"name\\":\\"{n}\\"' not in text]
        self.row(f"mcp.{key}.own-endpoints.{own}", org, f"tools/call jc_resource_list {own}",
                 f"every Endpoint of {own} ({len(self.exp[org]['projects'][own]['Endpoint'])})",
                 f"{st}, missing {missing[:4]}", st == 200 and not missing)
        for name in ("jc_endpoint_list_all", "jc_person_list", "jc_member_list", "jc_organization_read",
                     "jc_catalog_search", "jc_permissions_me", "jc_workspace_list", "jc_activity_list"):
            if name not in self.people[key].get("tools", []):
                continue
            args = {"project": own} | {"jc_catalog_search": {"q": "air"},
                                       "jc_organization_read": {"aspect": "projects"}}.get(name, {})
            st, res = call(name, args)
            r = (res or {}).get("result") or {}
            text = json.dumps(res, ensure_ascii=False)
            found = leaks(text)
            if installation_admin and name in ("jc_person_list", "jc_member_list"):
                found = [m for m in found if not m.startswith("mo-")]
            # A refusal for want of a right is fine here; a refused argument would make the row vacuous.
            status = (r.get("structuredContent") or {}).get("status")
            self.row(f"mcp.{key}.{name}", org, f"tools/call {name} {json.dumps(args)}",
                     "answers or refuses (403/404), names no object of another org",
                     f"{st}, isError={r.get('isError')}, foreign {found[:4]}" + (f", {r.get('structuredContent', {}).get('detail', '')[:80]}" if r.get("isError") else ""),
                     st == 200 and not found and (not r.get("isError") or status in DENIED))
        for other, oe in self.exp.items():
            if other == org:
                continue
            project = sorted(oe["projects"])[0]
            for name, args in (("jc_resource_list", {"project": project, "kind": "Endpoint"}),
                               ("jc_project_get", {"project": project}),
                               ("jc_change_list", {"project": project}),
                               ("jc_organization_read", {"organization": other})):
                if name not in self.people[key].get("tools", []):
                    continue
                st, res = call(name, args)
                r = (res or {}).get("result") or {}
                refused = st in DENIED or "error" in (res or {}) or bool(r.get("isError"))
                found = leaks(json.dumps(res, ensure_ascii=False), skip={project, other})
                self.row(f"mcp.{key}.{name}.{other}", f"{org}->{other}", f"tools/call {name} {json.dumps(args)}",
                         "refused, no foreign object named", f"{st}, refused={refused}, foreign {found[:4]}",
                         refused and not found)

    def check_catalogue(self):
        """Each Organization's CKAN organisations hold only its datasets, in search, page and feed."""
        h = self.client()
        st, body = h.json("GET", "/opendata/api/3/action/package_search?rows=1000&include_private=false")
        results = (body or {}).get("result", {}).get("results", []) if st == 200 else []
        self.row("catalogue.datasets-published", "dataroom", "GET /opendata/api/3/action/package_search",
                 "> 0 datasets", f"{st}, {len(results)}", st == 200 and len(results) > 0)
        for org, e in self.exp.items():
            mine = {c for p in e["projects"].values() for c in p["ckanOrgs"]}
            others = {c for o, oe in self.exp.items() if o != org for p in oe["projects"].values() for c in p["ckanOrgs"]}
            for c in sorted(mine):
                q = urllib.parse.quote(f'organization:"{c}"')
                st, body = h.json("GET", f"/opendata/api/3/action/package_search?fq={q}&rows=1000")
                rs = (body or {}).get("result", {}).get("results", []) if st == 200 else []
                wrong = sorted({(r.get("organization") or {}).get("name") for r in rs} - {c})
                self.row(f"catalogue.{org}.{c}.search", org, f"package_search fq=organization:{c}",
                         f"only {c}", f"{st}, {len(rs)} datasets, other orgs {wrong}", st == 200 and not wrong)
                names = {r.get("name") for r in rs}
                for kind, path in (("page", f"/opendata/organization/{c}"),
                                   ("dcat", f"/opendata/publisher/{c}/catalog.jsonld")):
                    st, _, raw = h.call("GET", path)
                    text = raw.decode(errors="replace")
                    other_names = {r.get("name") for r in results
                                   if (r.get("organization") or {}).get("name") in others}
                    leaked = sorted(n for n in other_names if n and re.search(rf"\b{re.escape(n)}\b", text))
                    self.row(f"catalogue.{org}.{c}.{kind}", org, f"GET {path}", "200, no dataset of another org",
                             f"{st}, {len(names)} own, leaked {leaked[:3]}", st == 200 and not leaked)

    def run(self):
        self.expectations()
        if not self.state:
            self.row("setup", "all", "-", "--state", "none", None, "no --state: persons cannot be made")
            return
        try:
            self.setup()
        except Exception as ex:  # noqa: BLE001 - a setup failure is a row, the rest is skipped
            self.row("setup", "all", "persons and bindings", "made", str(ex)[:300], False)
            return
        steps = (self.check_projects, self.check_project_reads, self.check_writes, self.check_people,
                 self.check_mcp, self.check_catalogue)
        for step in steps:
            if self.a.only and step.__name__.removeprefix("check_") not in self.a.only.split(","):
                continue
            try:
                step()
            except Exception as ex:  # noqa: BLE001
                self.row(f"{step.__name__}.crashed", "all", step.__name__, "runs", repr(ex)[:300], False)

    def write(self, out: Path):
        out.mkdir(parents=True, exist_ok=True)
        (out / "results.json").write_text(json.dumps(self.rows, indent=1))
        tools = {k: v.get("tools") for k, v in self.people.items() if v.get("tools") is not None}
        (out / "mcp-tools.json").write_text(json.dumps(tools, indent=1))
        n = {r: sum(1 for x in self.rows if x["result"] == r) for r in ("pass", "fail", "skip")}
        lines = ["# multiorg smoke", "", f"Base {self.base}; organizations {', '.join(self.exp)}; "
                 f"{n['pass']} pass, {n['fail']} fail, {n['skip']} skip.", "",
                 "| id | target | call | expected | actual | result |", "|---|---|---|---|---|---|"]
        order = {"fail": 0, "skip": 1, "pass": 2}
        for r in sorted(self.rows, key=lambda x: order[x["result"]]):
            cell = lambda s: str(s).replace("|", "\\|").replace("\n", " ")[:200]  # noqa: E731
            lines.append(f"| {cell(r['id'])} | {cell(r['target'])} | `{cell(r['call'])}` | {cell(r['expected'])} | "
                         f"{cell(r['actual'])} | {r['result']}{' (' + cell(r['reason']) + ')' if r['reason'] else ''} |")
        (out / "REPORT.md").write_text("\n".join(lines) + "\n")
        return n


def main(argv):
    ap = argparse.ArgumentParser(prog="run multiorg")
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--export", required=True, help="bundle written by compose.py")
    ap.add_argument("--state", help="the rehearsal's state dir (realm admin client, forge token)")
    ap.add_argument("--admin-token-file", help="unused: persons are made through --state")
    ap.add_argument("--person-token-file", help="unused: persons are made through --state")
    ap.add_argument("--only", help="comma list of check groups: projects,project_reads,writes,people,mcp,catalogue")
    ap.add_argument("--sync-timeout", type=int, default=180)
    a = ap.parse_args(argv)
    R.loopback_only(a.base_url)
    s = Suite(a)
    s.run()
    n = s.write(a.out)
    print(json.dumps(n))
    return 1 if n["fail"] else 0
