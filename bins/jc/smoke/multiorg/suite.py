"""Smoke suite `multiorg`: one jc serving several Organizations, each isolated from the others.

    bins/jc/smoke/run multiorg --base-url http://localhost:42301 --state <rehearsal state> \
        --export <bundle composed by compose.py> --out <dir>

Expectations come from the bundle (manifest.json `multiorg`, the project repositories' manifests).
--state is the rehearsal's state dir (rehearsal.py): the realm admin client and the forge token
there make the test persons (`mo-*`, random passwords kept in memory) and their RoleBindings.

Personas: `mo-platform` (group platform-admins) and, per Organization O, `mo-O-admin` (RoleBinding
role org-admin, scope organization O) and `mo-O-member` (role viewer, scope O). Each foreign read is
paired with a control: the same call by the owner Organization's admin must answer 200, so a 404
that means "no such route" never passes for isolation.

Host mode (--hosts <jc.toml> naming [[organizations]], alias --jc-config): each Organization's people sign in and
call on its own Portal host, every request sent to --base-url with the Host header of the address
(as APISIX passes it); kaisatsu stays on the base host. `check_hosts` adds what only hosts can
show: a person of A signed in on B's host (SSO) gets 403, A's cookie on B's host is no session,
the platform admin on a host sees that Organization alone, the gateway and open data hosts of one
link and list nothing of another, a host outside the registry is 421, B's prefix on A's host is a
404 and A's own prefixed form answers in place (API) or 308 (page)."""
import argparse
import base64
import hashlib
import json
import re
import secrets
import sys
import time
import tomllib
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "deploy" / "rehearsal"))
import rehearsal as R  # noqa: E402  (Http, sign-in, person helpers of the rehearsal kit)
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import realm_of  # noqa: E402

CONFIG = ("joinedcontext", "configuration")
DENIED = (403, 404)
MCP_CLIENT = "mcp-local"  # the realm's public client for a local MCP client
# Project-scoped reads every Organization's people use; each is checked both ways.
PROJECT_READS = ["", "/spaces", "/endpoints", "/pipelines", "/apps", "/datamodels", "/datasources",
                 "/changes", "/dashboards", "/policies", "/rolebindings", "/ckaninstances",
                 "/serviceaccounts", "/activity", "/syncsources", "/mqttbrokers", "/drafts"]


def organization_hosts(jc_toml: Path) -> dict:
    """org -> {portal, opendata, api, mcp} of a jc.toml's [[organizations]]."""
    orgs = tomllib.loads(jc_toml.read_text()).get("organizations", [])
    return {o["name"]: {k: o["hosts"][k].lower() for k in ("portal", "opendata", "api", "mcp")} for o in orgs}


class HostHttp(R.Http):
    """A browser on one jc's Organization hosts: each request goes to the loopback base with the
    Host header of its address, as APISIX passes it, and each host keeps its own cookie jar (a
    `__Host-` cookie is the host's alone). A relative path is on `host`; an absolute address on a
    host this jc does not serve is refused, so nothing leaves loopback."""

    def __init__(self, base: str, host: str | None = None, hosts=()):
        super().__init__(base)
        self.origin = urllib.parse.urlparse(self.base)
        self.host = host or self.origin.netloc
        self.hosts = {self.origin.netloc, *hosts}
        self.jars: dict[str, dict] = {}

    def url(self, path: str) -> str:
        return path if path.startswith("http") else f"{self.origin.scheme}://{self.host}{path}"

    def wire(self, path: str) -> tuple[str, str]:
        u = urllib.parse.urlparse(self.url(path))
        if u.netloc not in self.hosts:
            raise RuntimeError(f"{u.netloc} is not a host of this jc")
        rest = u.path + (f"?{u.query}" if u.query else "")
        return u.netloc, f"{self.origin.scheme}://{self.origin.netloc}{rest}"

    def call(self, method, path, body=None, headers=None, form=None, timeout=60):
        netloc, wire = self.wire(path)
        self.jar = self.jars.setdefault(netloc, {})
        return super().call(method, wire, body, {**(headers or {}), "Host": netloc}, form, timeout)


def claims(token: str) -> dict:
    """The payload of a JWT, unverified: only to read which audience kaisatsu wrote."""
    part = token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))


class Suite:
    def __init__(self, a):
        self.a = a
        self.rows = []
        self.base = a.base_url.rstrip("/")
        self.state = Path(a.state) if a.state else None
        self.realm = realm_of(self.state)
        self.exp: dict = {}
        self.export = Path(a.export)
        self.people: dict[str, dict] = {}
        self.dashboards: dict[str, tuple[str, str]] = {}
        self.exchange_ok = False  # an own control exchange succeeded: a foreign refusal then counts  # org -> (project, name) the suite committed
        self.hosts = organization_hosts(Path(a.jc_config)) if a.jc_config else {}

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
                kinds = {"Endpoint": [], "Pipeline": [], "CkanInstance": [], "Dashboard": []}
                for f in (files / CONFIG[0] / p).rglob("*.yaml"):
                    text = f.read_text(errors="replace")
                    for kind in kinds:
                        if re.search(rf"^kind:\s*{kind}\s*$", text, re.M):
                            n = re.search(r"^metadata:\s*\n(?:\s+.*\n)*?\s+name:\s*([\w.-]+)", text, re.M)
                            if n:
                                kinds[kind].append(n.group(1))
                closed = {}  # slug -> (name, audience) of every Endpoint that is not public
                for f in (files / CONFIG[0] / p).glob("spaces/*/endpoints/*.yaml"):
                    text = f.read_text(errors="replace")
                    aud = re.search(r"^\s*audience:\s*([\w-]+)", text, re.M)
                    slug = re.search(r"^\s*slug:\s*([a-z2-7]+)", text, re.M)
                    name = re.search(r"^metadata:\s*\n(?:\s+.*\n)*?\s+name:\s*([\w.-]+)", text, re.M)
                    if aud and slug and name and aud.group(1) != "public":
                        closed[slug.group(1)] = (name.group(1), aud.group(1))
                ckan = []
                for f in (files / CONFIG[0] / p / "ckan").glob("*.yaml"):
                    d = re.search(r"organizationDefault:\s*([\w.-]+)", f.read_text())
                    if d:
                        ckan.append(d.group(1))
                exp[org]["projects"][p] = {k: sorted(set(v)) for k, v in kinds.items()} | {"ckanOrgs": ckan,
                                                                                          "closed": closed}
        self.exp = exp
        orgs = files / CONFIG[0] / CONFIG[1] / "organizations"
        self.installation = next((f.stem for f in orgs.glob("*.yaml")
                                  if re.search(r"^\s*installation:\s*true", f.read_text(), re.M)), None)
        return exp

    # ---- personas
    def client(self, org: str | None = None, kind: str = "portal") -> R.Http:
        """Path mode: the base. Host mode: `org`'s `kind` host (the base without an org)."""
        if not self.hosts:
            return R.Http(self.base)
        every = [h for o in self.hosts.values() for h in o.values()]
        return HostHttp(self.base, self.hosts[org][kind] if org else None, every)

    def home(self, key: str) -> str | None:
        """The Organization whose Portal host persona `key` works on in host mode: its own; the
        platform admin the installation's."""
        if not self.hosts:
            return None
        return self.installation if key == "platform" else key.rsplit("-", 1)[0]

    def setup(self):
        h = self.client()
        tok = R.admin_token(h, self.state)
        names = {"platform": ["/platform-admins"]}
        for org in self.exp:
            names[f"{org}-admin"] = []
            names[f"{org}-member"] = []
        for key, groups in names.items():
            uid, pw = R.make_person(h, tok, self.realm, f"mo-{key}", groups=groups)
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
        for org, e in self.exp.items():
            subjects = "".join(f"    - id: {self.people[f'{org}-{k}']['id']}\n      kind: User\n" for k in ("admin", "member"))
            files.append((f"users/delegation/multiorg-{org}.yaml",
                          "apiVersion: joinedcontext.com/v1alpha1\nkind: DelegationPolicy\nmetadata:\n"
                          f"  name: multiorg-{org}\n  namespace: org\nspec:\n  aboveCeiling: refuse\n"
                          f"  maxExpiry: P30D\n  projects: [{', '.join(sorted(e['projects']))}]\n"
                          f"  segments: [data-read, context]\n  subjects:\n{subjects}  writes: false\n"))
        self.commit(files, "multiorg smoke: test persons' RoleBindings and DelegationPolicies")
        # One organization-wide Dashboard per Organization, in its first project with an Endpoint
        # (the exports hold Dashboards only for helsinki).
        for org, e in self.exp.items():
            project = next((p for p in sorted(e["projects"]) if e["projects"][p]["Endpoint"]), None)
            if not project:
                continue
            endpoint = sorted(e["projects"][project]["Endpoint"])[0]
            self.dashboards[org] = (project, f"multiorg-{org}")
            self.commit([(f"dashboards/multiorg-{org}.yaml",
                          "apiVersion: joinedcontext.com/v1alpha1\nkind: Dashboard\nmetadata:\n"
                          f"  name: multiorg-{org}\n  namespace: {project}\nspec:\n"
                          f"  title: multiorg {org}\n  visibility: organization\n  pages:\n"
                          f"    - title: Overview\n      widgets:\n        - widgetType: temporal-chart\n"
                          f"          endpointRef: {endpoint}\n")],
                        f"multiorg smoke: a Dashboard of {org}", repo=("joinedcontext", project))
        # Wait until each admin sees its Organization's projects (the Portal's sync).
        deadline = time.time() + self.a.sync_timeout
        pending = set(self.exp)
        while pending and time.time() < deadline:
            for org in sorted(pending):
                st, body = self.session(f"{org}-admin").json("GET", "/api/v1/projects")
                st_d, _ = self.session("platform").json("GET", f"/api/v1/projects/org/delegationpolicies/multiorg-{org}")
                dash = self.dashboards.get(org)
                st_b = self.session("platform", org if self.hosts else None).json("GET", f"/api/v1/projects/{dash[0]}/dashboards/{dash[1]}")[0] if dash else 200
                if st == 200 and st_d == 200 and st_b == 200 and self.bound(org) >= {f"mo-{org}-admin", f"mo-{org}-member"} and \
                        set(self.exp[org]["projects"]) <= {i["name"] for i in body.get("items", [])}:
                    pending.discard(org)
            if pending:
                time.sleep(5)
        # Sessions and tokens taken while waiting predate the {org}_bound groups (jc_org): every
        # check signs in again.
        for p in self.people.values():
            p.pop("http", None)
            p.pop("tokens", None)
        self.row("setup.bindings-synced", ",".join(self.exp), "GET /api/v1/projects (each org admin)",
                 "every admin lists its projects; DelegationPolicy, Dashboard and {org}_bound group synced", f"not yet: {sorted(pending)}" if pending else "all",
                 not pending)

    def bound(self, org) -> set:
        """Usernames in the reconciler's group `{org}_bound` (jc_org of org-scoped bindings)."""
        h = self.client()
        auth = {"Authorization": f"Bearer {R.admin_token(h, self.state)}"}
        st, groups = h.json("GET", f"/id/admin/realms/{self.realm}/groups?search={org}_bound", headers=auth)
        g = next((g for g in groups or [] if isinstance(g, dict) and g.get("name") == f"{org}_bound"), None)
        if st != 200 or not g:
            return set()
        st, members = h.json("GET", f"/id/admin/realms/{self.realm}/groups/{g['id']}/members", headers=auth)
        return {m["username"] for m in members} if st == 200 and isinstance(members, list) else set()

    def commit(self, files, message, repo=CONFIG):
        h = self.client()
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        owner, repo = repo
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
        if ops:  # through a pull request: the imported main takes no direct push
            R.merge_change(lambda m, path, body: h.json(m, f"/git/api/v1{path}", headers=auth, body=body),
                           owner, repo, ops, message)

    def session(self, key, on: str | None = None) -> R.Http:
        """Persona `key` signed in; in host mode on its own Organization's Portal host, or on
        `on`'s (the platform admin works on each Organization's host in turn)."""
        p = self.people[key]
        org = on or self.home(key)
        held = p.setdefault("http", {})
        if org not in held:
            h = self.client(org)
            ok, detail = R.sign_in(h, p["username"], p["password"])
            if not ok:
                raise RuntimeError(f"sign-in of {p['username']}{f' on {org}' if org else ''}: {detail}")
            held[org] = h
        return held[org]

    def bearer(self, key, scope="mcp:portal", client=None) -> str | None:
        """An MCP access token of the person, as a local MCP client gets one: code flow with PKCE
        through the realm's public client mcp-local, scope mcp:portal (audience portal-api); the
        redirect to the loopback callback is read, never followed."""
        p = self.people[key]
        held = p.setdefault("tokens", {})
        if scope in held:
            return held[scope]
        # A confidential client (the Portal's own) redirects to the Portal's callback, read here.
        client_id, secret_file = client or (MCP_CLIENT, None)
        h = self.client()
        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        redirect = f"{self.base}/api/v1/auth/callback" if secret_file else "http://127.0.0.1/callback"
        q = urllib.parse.urlencode({"client_id": client_id, "redirect_uri": redirect, "response_type": "code",
                                    "scope": f"openid {scope}", "state": "mo", "code_challenge": challenge,
                                    "code_challenge_method": "S256"})
        login_url, st, page = R.follow(h, f"/id/realms/{self.realm}/protocol/openid-connect/auth?{q}")
        held[scope] = None
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
        st, tok = h.json("POST", f"/id/realms/{self.realm}/protocol/openid-connect/token", form={
            "grant_type": "authorization_code", "client_id": client_id, "code": code,
            "redirect_uri": redirect, "code_verifier": verifier,
            **({"client_secret": R.read_secret(self.state, secret_file)} if secret_file else {})})
        held[scope] = tok.get("access_token") if st == 200 and isinstance(tok, dict) else None
        return held[scope]

    # ---- checks
    def check_projects(self):
        everything = {p for o in self.exp.values() for p in o["projects"]}
        st, body = self.session("platform").json("GET", "/api/v1/projects")
        seen = {i["name"] for i in (body or {}).get("items", [])} if st == 200 else set()
        if self.hosts:  # on a host the platform admin works in that Organization alone (check_hosts)
            mine = set(self.exp[self.installation]["projects"])
            self.row("projects.platform-on-installation-host", "platform", "GET /api/v1/projects",
                     f"{len(mine)} projects of {self.installation}, no other",
                     f"{st}, missing {sorted(mine - seen)}, foreign {sorted(seen & (everything - mine))}",
                     st == 200 and mine <= seen and not seen & (everything - mine))
        else:
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
        """The members list of an org admin names its own people and nobody bound only to another
        Organization. The installation Organization's admins hold the realm-wide Person read
        (ADR-N-058 §2.2): their row is a skip naming that, for the owner to decide."""
        for org in self.exp:
            st, body = self.session(f"{org}-admin").json("GET", "/api/v1/organization/people")
            emails = {i.get("email", "") for i in (body or {}).get("items", [])} if st == 200 else set()
            own = {f"mo-{org}-admin@rehearsal.invalid", f"mo-{org}-member@rehearsal.invalid"}
            foreign = sorted(x for x in emails if (m := re.match(r"mo-(.+)-(admin|member)@", x)) and m.group(1) != org)
            # The list is realm-wide (people.rs list_people asks read on Person at the installation):
            # an admin of another Organization is refused it and has no list of its own members.
            gap = org != self.installation and st in DENIED
            self.row(f"people.{org}-admin.own", org, "GET /api/v1/organization/people", f"{sorted(own)} listed",
                     f"{st}, missing {sorted(own - emails)}", None if gap else (st == 200 and own <= emails),
                     "gap: the Portal has no per-Organization people list; the realm-wide one is the "
                     "installation's (ADR-N-058 2.2), so this admin cannot list its own members" if gap else "")
            if org == self.installation:
                self.row(f"people.{org}-admin.no-foreign", org, "GET /api/v1/organization/people",
                         "nobody of another org", f"{st}, foreign {foreign}", None if foreign else st == 200,
                         "installation admin: realm-wide Person read (ADR-N-058 §2.2); lists other "
                         "Organizations' people: owner decision" if foreign else "")
            else:
                self.row(f"people.{org}-admin.no-foreign", org, "GET /api/v1/organization/people",
                         "nobody of another org", f"{st}, foreign {foreign}", st in (200, *DENIED) and not foreign)
            st, body = self.session(f"{org}-member").json("GET", "/api/v1/organization/people")
            emails = json.dumps(body)
            foreign = sorted({m.group(1) for m in re.finditer(r"mo-([a-z]+)-(?:admin|member)@", emails)} - {org})
            self.row(f"people.{org}-member", org, "GET /api/v1/organization/people", "403/404 or nobody of another org",
                     f"{st}, foreign {foreign}", st in DENIED or (st == 200 and not foreign))

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
                h = self.client(self.home(key))
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

    def check_connectors(self):
        """Each org admin's connector, scoped to its own project, is invisible and untouchable to
        the other Organizations' people; a connector scoped to a foreign project is refused."""
        made = {}
        expires = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 7 * 86400))
        for org, e in self.exp.items():
            own = sorted(e["projects"])[0]
            h = self.session(f"{org}-admin")
            st, body = h.json("POST", "/api/v1/connectors", body={
                "name": f"mo-{org}-{int(time.time())}", "segments": ["data-read", "context"], "expiresAt": expires,
                "scope": [{"project": own, "access": "read"}]})
            cid = (body or {}).get("id") if isinstance(body, dict) else None
            self.row(f"connectors.{org}-admin.create", org, "POST /api/v1/connectors", "201 with id",
                     f"{st} {str(body)[:160] if not cid else cid}", st in (200, 201) and bool(cid))
            if cid:
                made[org] = (cid, own)
            for other, oe in self.exp.items():
                if other == org:
                    continue
                foreign = sorted(oe["projects"])[0]
                st, body = h.json("POST", "/api/v1/connectors", body={
                    "name": f"mo-{org}-into-{other}", "segments": ["data-read", "context"], "expiresAt": expires,
                    "scope": [{"project": foreign, "access": "read"}]})
                if st in (200, 201) and isinstance(body, dict) and body.get("id"):
                    h.call("DELETE", f"/api/v1/connectors/{body['id']}")
                # Only the scope differs from the own connector's body: without that one, a 4xx
                # proves nothing.
                self.row(f"connectors.{org}-admin.create-into.{foreign}", f"{org}->{other}",
                         f"POST /api/v1/connectors scope {foreign}", "4xx (own create 201)", f"{st} {str(body)[:160]}",
                         (400 <= st < 500) if cid else None, "" if cid else "own create failed: no control")
        for owner, (cid, own) in made.items():
            for other in self.exp:
                if other == owner:
                    continue
                for kind in ("admin", "member"):
                    h = self.session(f"{other}-{kind}")
                    st, _, _ = h.call("GET", f"/api/v1/connectors/{cid}")
                    st_list, lst = h.json("GET", "/api/v1/connectors")
                    listed = cid in json.dumps(lst)
                    st_del, _, _ = h.call("DELETE", f"/api/v1/connectors/{cid}")
                    st_act, _, _ = h.call("GET", f"/api/v1/connectors/{cid}/activity")
                    self.row(f"connectors.{other}-{kind}.on.{owner}", f"{other}->{owner}",
                             f"GET, DELETE, GET activity /api/v1/connectors/{cid}; GET /api/v1/connectors",
                             "404/403 each, not listed", f"get {st}, delete {st_del}, activity {st_act}, listed {listed}",
                             st in DENIED and st_del in DENIED and st_act in DENIED and not listed)
                    tok = self.bearer(f"{other}-{kind}", f"connector:{cid}")
                    if tok:
                        st, res = self.client(self.home(f"{other}-{kind}")).json("POST", f"/c/{cid}", headers={
                            "Authorization": f"Bearer {tok}", "Accept": "application/json, text/event-stream"},
                            body={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
                        self.row(f"connectors.{other}-{kind}.mcp.{owner}", f"{other}->{owner}",
                                 f"POST /c/{cid} tools/list", "401/403/404", f"{st}", st in (401, *DENIED))
            st, _, _ = self.session(f"{owner}-admin").call("GET", f"/api/v1/connectors/{cid}")
            self.row(f"connectors.{owner}-admin.still-there", owner, f"GET /api/v1/connectors/{cid}", "200",
                     f"{st}", st == 200)
            tok = self.bearer(f"{owner}-admin", f"connector:{cid}")
            if not tok:
                self.row(f"connectors.{owner}-admin.mcp", owner, f"POST /c/{cid}", "a connector token", "none",
                         None, f"code flow with scope connector:{cid} through mcp-local gave no token")
                continue
            h = self.client(self.home(f"{owner}-admin"))
            auth = {"Authorization": f"Bearer {tok}", "Accept": "application/json, text/event-stream"}
            st, init = h.json("POST", f"/c/{cid}", headers=auth, body={
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "multiorg-smoke", "version": "1"}}})
            st2, lst = h.json("POST", f"/c/{cid}", headers=auth,
                              body={"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
            tools = sorted(t["name"] for t in ((lst or {}).get("result") or {}).get("tools", [])) if st2 == 200 else []
            self.people[f"{owner}-admin"]["connectorTools"] = tools
            found = [m for m in self.foreign_marks(owner) if m in json.dumps(init) + json.dumps(lst)]
            self.row(f"connectors.{owner}-admin.mcp", owner, f"POST /c/{cid} initialize + tools/list",
                     "200, tools, no foreign object named", f"{st}/{st2}, {len(tools)} tools, foreign {found[:3]}",
                     st == 200 and st2 == 200 and bool(tools) and not found)

    def check_data(self):
        """Endpoints that are not public: another Organization's people never see them in the
        readable catalogue or the MCP hub, and never read them through the gateway."""
        for org in self.exp:
            foreign = {slug: (p, *v) for o, oe in self.exp.items() if o != org
                       for p, pe in oe["projects"].items() for slug, v in pe["closed"].items()}
            own_org = {slug for p, pe in self.exp[org]["projects"].items()
                       for slug, (_, aud) in pe["closed"].items() if aud == "organization"}
            for kind in ("admin", "member"):
                key = f"{org}-{kind}"
                st, body = self.session(key).json("GET", "/api/v1/catalogue/readable")
                slugs = {d.get("slug") for d in (body or {}).get("datasets", [])} if st == 200 else set()
                seen = sorted(f"{foreign[x][0]}/{foreign[x][1]}" for x in slugs & set(foreign))
                self.row(f"data.{key}.readable.no-foreign", org, "GET /api/v1/catalogue/readable",
                         "no closed Endpoint of another org", f"{st}, {len(slugs)} listed, foreign {seen[:4]}",
                         st == 200 and not seen)
                if own_org:
                    # `audience: organization` admits the Organization; a Policy still has to name the
                    # caller, and the export's name project groups, not these persons: at least one.
                    got = own_org & slugs
                    self.row(f"data.{key}.readable.own-organization", org, "GET /api/v1/catalogue/readable",
                             f">= 1 of its {len(own_org)} organization Endpoints (the rest are Policy-gated)",
                             f"{st}, {len(got)} listed", st == 200 and bool(got))
                # The Portal's data path for a person (auth/oidc.rs exchange_for_endpoint): the
                # person's token exchanged for one Endpoint, audience mcp-hub, scope endpoint:{slug}.
                # Another Organization's closed Endpoint: the realm refuses the exchange or the
                # gateway the read. An own readable one is the control.
                control = sorted(got)[:1] if own_org else []
                for slug in control:
                    st, tok = self.exchange(key, slug)
                    code = self.client(self.home(key), "api").call("GET", f"/api/endpoint/{slug}/ngsi-ld/v1/types",
                                              headers={"Authorization": f"Bearer {tok}"})[0] if tok else st
                    self.exchange_ok = self.exchange_ok or tok is not None
                    self.row(f"data.{key}.read.own", org, f"exchange endpoint:{slug}, GET /api/endpoint/{slug}/ngsi-ld/v1/types",
                             "200 (an own Endpoint the readable list names)", f"exchange {st}, read {code}", code == 200 if tok else None,
                             "" if tok else "the realm refused the exchange: no endpoint:{slug} scope linked to "
                             "portal-api (dev links them by its admin-permissions CronJob; jc has none)")
                for slug, (p, name, aud) in sorted(foreign.items())[:6]:
                    st, tok = self.exchange(key, slug)
                    code = self.client(self.home(key), "api").call("GET", f"/api/endpoint/{slug}/ngsi-ld/v1/types",
                                              headers={"Authorization": f"Bearer {tok}"})[0] if tok else None
                    self.row(f"data.{key}.read.{p}/{name}", org,
                             f"exchange endpoint:{slug}, GET /api/endpoint/{slug}/ngsi-ld/v1/types",
                             f"exchange refused or read 401/403/404 ({aud} Endpoint of another org)",
                             f"exchange {st}, read {code}",
                             (code in (401, *DENIED)) if tok else (True if self.exchange_ok else None),
                             "" if tok or self.exchange_ok else "the realm refuses every exchange here, the own "
                             "control too: a refusal proves nothing")

    def exchange(self, key, slug):
        """(status, token) of the Portal's token exchange of the person's mcp:portal token for one
        Endpoint; token None when the realm refused."""
        subject = self.bearer(key)
        if not subject:
            return None, None
        st, tok = self.client().json("POST", f"/id/realms/{self.realm}/protocol/openid-connect/token", form={
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "client_id": "portal-api", "client_secret": R.read_secret(self.state, "portal-client-secret"),
            "subject_token": subject, "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "audience": "mcp-hub", "scope": f"gateway-hub endpoint:{slug}"})
        return st, (tok.get("access_token") if st == 200 and isinstance(tok, dict) else None)

    def check_catalogue(self):
        """Each Organization's CKAN organisations hold only its datasets, in search, page and feed."""
        if self.hosts:
            return self.check_catalogue_hosts()
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
                st, body = h.json("GET", f"/api/v1/catalogue?publisher={urllib.parse.quote(c)}")
                ds = (body or {}).get("datasets", []) if st == 200 else []
                wrong = sorted({(d.get("publisher") or {}).get("name") for d in ds} - {c})
                self.row(f"catalogue.{org}.{c}.portal", org, f"GET /api/v1/catalogue?publisher={c}",
                         f"200, {len(rs)} datasets, only {c}",
                         f"{st}, total {(body or {}).get('total')}, other publishers {wrong}",
                         st == 200 and not wrong and (bool((body or {}).get("total")) or not rs))
                for kind, path in (("page", f"/opendata/organization/{c}"),
                                   ("dcat", f"/opendata/publisher/{c}/catalog.jsonld")):
                    st, _, raw = h.call("GET", path)
                    text = raw.decode(errors="replace")
                    other_names = {r.get("name") for r in results
                                   if (r.get("organization") or {}).get("name") in others}
                    leaked = sorted(n for n in other_names if n and re.search(rf"dataset/{re.escape(n)}(?![\w-])", text))
                    empty = st == 404 and not names and not leaked
                    self.row(f"catalogue.{org}.{c}.{kind}", org, f"GET {path}", "200, no dataset of another org",
                             f"{st}, {len(names)} own, leaked {leaked[:3]}",
                             None if empty else (st == 200 and not leaked),
                             f"no dataset published to {c} yet: dataroom has no such organisation" if empty else "")

    def check_catalogue_hosts(self):
        """Host mode: each Organization's open data host is its own catalogue, the Portal publishes
        its Endpoints there under the CKAN organisation of its name, and it lists, searches and
        feeds nothing of another's."""
        found = {}
        for org in self.exp:
            h = self.client(org, "opendata")
            st, body = h.json("GET", "/api/3/action/package_search?rows=1000&include_private=false")
            rs = (body or {}).get("result", {}).get("results", []) if st == 200 else []
            found[org] = {r.get("name") for r in rs}
            wrong = sorted({(r.get("organization") or {}).get("name") for r in rs} - {org})
            publishes = any(p["Endpoint"] for p in self.exp[org]["projects"].values())
            self.row(f"catalogue.{org}.published", org, "GET {opendata}/api/3/action/package_search",
                     f"> 0 datasets, every one in CKAN organisation {org}", f"{st}, {len(rs)}, other orgs {wrong}",
                     st == 200 and not wrong and (bool(rs) or not publishes))
            st, body = h.json("GET", "/api/3/action/organization_list")
            names = (body or {}).get("result", []) if st == 200 else []
            self.row(f"catalogue.{org}.organisations", org, "GET {opendata}/api/3/action/organization_list",
                     f"[{org}] alone", f"{st}, {names}", st == 200 and set(names) <= {org})
        for org in self.exp:
            h = self.client(org, "opendata")
            other_names = {n for o, ns in found.items() if o != org for n in ns} - found[org]
            for kind, path in (("search", "/api/3/action/package_search?rows=1000"), ("list", "/api/3/action/package_list"),
                               ("dcat", "/catalog.jsonld"), ("sitemap", "/sitemap.xml"), ("home", "/")):
                st, _, raw = h.call("GET", path)
                text = raw.decode(errors="replace")
                # A dataset by its identity: a name the API lists, or its page's link. A bare word
                # is no dataset (a barcelona dataset named `weather` beside hel's tag `weather`).
                if kind in ("search", "list"):
                    result = (json.loads(text or "{}") if st == 200 else {}).get("result", [])
                    listed = {r.get("name") for r in result.get("results", [])} if kind == "search" else set(result)
                    leaked = sorted(n for n in other_names if n in listed)
                else:
                    leaked = sorted(n for n in other_names if n and re.search(rf"/dataset/{re.escape(n)}(?![\w-])", text))
                hosts = sorted({hh for o, hs in self.hosts.items() if o != org for hh in hs.values() if hh in text})
                self.row(f"catalogue.{org}.{kind}.no-foreign", org, f"GET {{opendata}}{path}",
                         "200, no dataset or host of another org", f"{st}, leaked {leaked[:3]}, hosts {hosts}",
                         st == 200 and not leaked and not hosts)

    def check_hosts(self):
        """What only the Organizations' own hosts show (host mode)."""
        if not self.hosts:
            return
        everything = {p for o in self.exp.values() for p in o["projects"]}
        orgs = sorted(self.exp)
        for org in orgs:
            mine = set(self.exp[org]["projects"])
            # The platform admin on this host: this Organization alone (Decided: scoped).
            st, body = self.session("platform", org).json("GET", "/api/v1/projects")
            seen = {i["name"] for i in (body or {}).get("items", [])} if st == 200 else set()
            self.row(f"hosts.platform.{org}.projects", org, f"GET /api/v1/projects as the platform admin on {org}'s host",
                     f"all of {sorted(mine)}, nothing else", f"{st}, missing {sorted(mine - seen)}, foreign {sorted(seen - mine)}",
                     st == 200 and mine <= seen and not seen - mine)
            for other in orgs:
                if other == org:
                    continue
                foreign = sorted(self.exp[other]["projects"])[0]
                st, _, _ = self.session("platform", org).call("GET", f"/api/v1/projects/{foreign}")
                self.row(f"hosts.platform.{org}.foreign.{foreign}", f"{org}->{other}",
                         f"GET /api/v1/projects/{foreign} on {org}'s host as the platform admin", "403/404", st, st in DENIED)
                # SSO: other's admin, whose browser holds the realm session from signing in on its
                # own host, opens this host's sign-in: the realm answers at once, back to this host.
                st_own, _ = self.session(f"{other}-admin").json("GET", "/api/v1/projects")
                sso = self.session(f"{other}-admin")
                there = f"{urllib.parse.urlparse(self.base).scheme}://{self.hosts[org]['portal']}"
                url, st_l, _ = R.follow(sso, f"{there}/api/v1/auth/login")
                landed = urllib.parse.urlparse(url).netloc
                st, body = sso.json("GET", f"{there}/api/v1/projects")
                self.row(f"hosts.sso.{other}-admin.on.{org}", f"{other}->{org}",
                         f"GET {there}/api/v1/auth/login with the realm session, then GET /api/v1/projects",
                         f"back on {self.hosts[org]['portal']} without a sign-in page, then 403",
                         f"landed {landed} ({st_l}), {st} {str(body)[:100]}",
                         landed == self.hosts[org]["portal"] and st == 403)
                # A's cookie on B's host: the session and CSRF cookies of other's own host, replayed.
                own = self.session(f"{other}-admin")
                jar = own.jars.get(self.hosts[other]["portal"], {})
                cookie = "; ".join(f"{n}={v}" for n, (v, _) in jar.items())
                replay = self.client(org)
                st_r, _, _ = replay.call("GET", "/api/v1/projects", headers={"Cookie": cookie})
                st_w, _, _ = replay.call("POST", f"/api/v1/projects/{sorted(self.exp[org]['projects'])[0]}/workspaces",
                                         headers={"Cookie": cookie, "x-csrf-token": jar.get("__Host-jc_csrf", ("",))[0]},
                                         body={})
                self.row(f"hosts.cookie.{other}.on.{org}", f"{other}->{org}",
                         f"GET /api/v1/projects, POST workspaces on {org}'s host with {other}'s cookies",
                         "401 / 401-403 (control 200 on its own host)", f"read {st_r}, write {st_w}, control {st_own}",
                         st_own == 200 and st_r == 401 and st_w in (401, 403))
            # The gateway on this Organization's API host links its own hosts alone.
            api = self.client(org, "api")
            for path in ("/api/endpoints", "/catalog.jsonld", "/cs"):
                st, hdrs, raw = api.call("GET", path)
                for _ in range(3):  # a cold feed past the gateway's 10 s read limit: again, warm
                    if st != 503:
                        break
                    time.sleep(min(int((hdrs or {}).get("retry-after") or (hdrs or {}).get("Retry-After") or 2), 10))
                    st, hdrs, raw = api.call("GET", path)
                text = raw.decode(errors="replace")
                links = set(re.findall(r"https?://([a-z0-9.-]+(?::\d+)?)", text))
                theirs = sorted(l for l in links if l in {h for o, hs in self.hosts.items() if o != org for h in hs.values()}
                                or l == urllib.parse.urlparse(self.base).netloc)
                names = sorted(n for n in everything - mine if f'"{n}"' in text)
                self.row(f"hosts.gateway.{org}{path}", org, f"GET {{api}}{path}",
                         "200, links only its own hosts, names no foreign project",
                         f"{st}, foreign links {theirs[:3]}, foreign projects {names[:3]}",
                         st == 200 and not theirs and not names)
            self.check_prefixes(org, orgs)
        # A host outside the registry is 421 and names no Organization (docs/API/15-addresses-v1.md §2).
        st, _, raw = HostHttp(self.base, "nobody.localhost", ["nobody.localhost"]).call("GET", "/api/v1/projects")
        text = raw.decode(errors="replace")
        named = sorted(o for o in orgs if o in text)
        self.row("hosts.unknown", "-", "GET /api/v1/projects on a host nobody serves",
                 "421, no Organization named", f"{st}, names {named}", st == 421 and not named)

    def check_prefixes(self, org: str, orgs: list):
        """The prefixed form on `org`'s own bound hosts: the API answers in place, a page is 308 to
        the prefix-less form; another Organization's prefix there is the 404 of an unknown path."""
        data = self.client(org, "opendata")
        st, _, raw = data.call("GET", f"/{org}/api/3/action/package_list", headers={"Accept": "application/json"})
        st_c, _, raw_c = data.call("GET", "/api/3/action/package_list", headers={"Accept": "application/json"})
        self.row(f"hosts.prefix.{org}.api", org, f"GET {{opendata}}/{org}/api/3/action/package_list",
                 "200 in place, the same list as the prefix-less form", f"{st}, control {st_c}",
                 st == 200 and st_c == 200 and raw == raw_c)
        st, hdrs, _ = data.call("GET", f"/{org}/dataset?q=x", headers={"Accept": "text/html"})
        location = (hdrs or {}).get("location") or (hdrs or {}).get("Location")
        self.row(f"hosts.prefix.{org}.page", org, f"GET {{opendata}}/{org}/dataset?q=x as a page",
                 "308 to /dataset?q=x", f"{st} {location}", st == 308 and location == "/dataset?q=x")
        for other in orgs:
            if other == org:
                continue
            for kind, path in (("opendata", f"/{other}/api/3/action/package_list"),
                               ("api", f"/{other}/api/endpoints"), ("portal", f"/{other}/api/v1/projects")):
                st, _, raw = self.client(org, kind).call("GET", path, headers={"Accept": "application/json"})
                text = raw.decode(errors="replace")
                self.row(f"hosts.prefix.{org}.{kind}.{other}", f"{other}->{org}", f"GET {{{kind}}}{path} on {org}'s host",
                         "404 of an unknown path, naming nothing", f"{st} {text[:60]}",
                         st == 404 and other not in text)

    GLOBAL_READS = ["/api/v1/endpoints", "/api/v1/shared-apps", "/api/v1/connectors", "/api/v1/connectors/pending",
                    "/api/v1/blueprints", "/api/v1/alerts", "/api/v1/alerts/notices", "/api/v1/notifications",
                    "/api/v1/organization/agents", "/api/v1/organization/agents/activity",
                    "/api/v1/organization/brokers", "/api/v1/organization/connectors", "/api/v1/organization/costs",
                    "/api/v1/organization/data-access", "/api/v1/organization/datamodels",
                    "/api/v1/organization/feedback", "/api/v1/organization/health",
                    "/api/v1/organization/health-checks", "/api/v1/organization/limits",
                    "/api/v1/organization/setup"]

    def check_global(self):
        """The routes without a project (every parameterless GET of the Portal's OpenAPI that lists
        objects): a person of one Organization gets 403/404 or an answer naming nothing of another.
        The installation's administrator holds the realm-wide kinds (ADR-N-058 2.2): a foreign name
        in its answer is a skip naming the owner decision, never a pass."""
        def names_of(o):  # the marks that name Organization o, as another Organization sees them
            other = next(x for x in self.exp if x != o)
            return {m for m, v in self.foreign_marks(other).items() if v == o and not m.startswith("mo-")}
        if self.hosts:  # on each host the platform admin sees that Organization alone: the marks still work
            for org in self.exp:
                st, body = self.session("platform", org).json("GET", "/api/v1/endpoints")
                text = json.dumps(body, ensure_ascii=False)
                orgs_seen = sorted(o for o in self.exp if any(m in text for m in names_of(o)))
                # The organization-level Endpoints page is the installation's administration
                # (PF-61, ADR-N-058 2.2): on another Organization's host it is 404, as for that
                # Organization's own admin; where it answers, it names that Organization alone.
                home = org == self.installation
                self.row(f"global.platform.{org}./api/v1/endpoints", org,
                         f"GET /api/v1/endpoints as the platform admin on {org}'s host",
                         f"200 naming Endpoints of {org} alone" + ("" if home else ", or 404"),
                         f"{st}, orgs {orgs_seen}",
                         (st == 200 and orgs_seen == [org]) or (not home and st == 404))
        else:
            st, body = self.session("platform").json("GET", "/api/v1/endpoints")
            text = json.dumps(body, ensure_ascii=False)
            orgs_seen = sorted(o for o in self.exp if any(m in text for m in names_of(o)))
            self.row("global.platform./api/v1/endpoints", "all", "GET /api/v1/endpoints as the platform admin",
                     "200 naming Endpoints of every Organization (the marks work)", f"{st}, orgs {orgs_seen}",
                     st == 200 and orgs_seen == sorted(self.exp))
        for org in self.exp:
            marks = self.foreign_marks(org)
            for kind in ("admin", "member"):
                key = f"{org}-{kind}"
                for path in self.GLOBAL_READS:
                    st, body = self.session(key).json("GET", path)
                    text = json.dumps(body, ensure_ascii=False)
                    hits = sorted(m for m in marks if m in text) if st == 200 else []
                    ok = st in DENIED or (st == 200 and not hits)
                    reason = ""
                    if st == 503:  # a dependency this jc lacks: nothing answered, nothing leaked
                        ok, reason = None, f"503: {str((body or {}).get('detail', ''))[:120]}"
                    if hits and org == self.installation and kind == "admin":
                        ok, reason = None, ("installation admin: realm-wide kinds (ADR-N-058 2.2) name other "
                                            "Organizations' objects; owner decision")
                    self.row(f"global.{key}.{path}", org, f"GET {path} as mo-{key}",
                             "403/404, or 200 naming no object of another Organization",
                             f"{st}, foreign {hits[:4]}", ok, reason)

    def check_dashboards(self):
        """Each Organization's Dashboards (the export's and the suite's own) are listed for its
        people and refused to every other Organization's, item by item; the platform admin reads all."""
        for org, e in self.exp.items():
            want = {(p, d) for p, v in e["projects"].items() for d in v["Dashboard"]}
            if org in self.dashboards:
                want.add(self.dashboards[org])
            if not want:
                self.row(f"dashboards.{org}", org, "-", "a Dashboard", "none", None,
                         "no Dashboard in the export and no project with an Endpoint to add one")
                continue
            for p, d in sorted(want):
                for kind in ("admin", "member"):
                    st, body = self.session(f"{org}-{kind}").json("GET", f"/api/v1/projects/{p}/dashboards")
                    names = {i.get("metadata", {}).get("name", i.get("name")) for i in (body or {}).get("items", [])} \
                        if st == 200 and isinstance(body, dict) else set()
                    self.row(f"dashboards.{org}.{p}.{d}.own-{kind}", org, f"GET /api/v1/projects/{p}/dashboards",
                             f"200 listing {d}", f"{st}, listed {d in names}", st == 200 and d in names)
                path = f"/api/v1/projects/{p}/dashboards/{d}"
                st, _ = self.session("platform", org if self.hosts else None).json("GET", path)
                self.row(f"dashboards.{org}.{p}.{d}.platform", org, f"GET {path}", "200", st, st == 200)
                for other in self.exp:
                    if other == org:
                        continue
                    for kind in ("admin", "member"):
                        for call in (path, f"/api/v1/projects/{p}/dashboards"):
                            st, body = self.session(f"{other}-{kind}").json("GET", call)
                            leaked = st == 200 and d in json.dumps(body)
                            self.row(f"dashboards.{org}.{p}.{d}.foreign-{other}-{kind}{'' if call == path else '-list'}",
                                     org, f"GET {call} as mo-{other}-{kind}", "403/404",
                                     f"{st}{', names it' if leaked else ''}", st in DENIED)

    def check_ui(self):
        """Shots at 1440 and 400 px per persona: the project list, an own project and a foreign
        project's address, which must show no foreign project content."""
        from playwright.sync_api import sync_playwright  # only this step needs a browser

        out = self.a.out / "shots"
        out.mkdir(parents=True, exist_ok=True)
        everything = {p for o in self.exp.values() for p in o["projects"]}
        shots = []
        scheme = urllib.parse.urlparse(self.base).scheme
        # Host mode: every Organization host is this loopback jc (the port stays the address's).
        args = ["--disable-dev-shm-usage", "--disable-gpu"]
        if self.hosts:
            loop = urllib.parse.urlparse(self.base).hostname
            loop = "127.0.0.1" if loop == "localhost" else loop  # *.localhost is ::1 here
            names = sorted({h.split(":")[0] for o in self.hosts.values() for h in o.values()})
            args.append("--host-resolver-rules=" + ",".join(f"MAP {n} {loop}" for n in names))
        with sync_playwright() as pw:
            browser = pw.chromium.launch(args=args)
            for org, h in self.hosts.items():
                for width, height in ((1440, 900), (400, 860)):
                    ctx = browser.new_context(viewport={"width": width, "height": height})
                    page = ctx.new_page()
                    url = f"{scheme}://{h['opendata']}/"
                    page.goto(url, wait_until="networkidle", timeout=60000)
                    page.wait_for_timeout(1500)
                    file = out / f"{org}-opendata-home-{width}.png"
                    page.screenshot(path=str(file), full_page=False)
                    text = page.locator("body").inner_text(timeout=5000).lower()
                    shots.append({"persona": "anonymous", "page": "opendata-home", "path": url,
                                  "width": width, "file": file.name})
                    leaked = sorted(o for o in self.hosts if o != org and self.hosts[o]["opendata"] in text)
                    self.row(f"ui.{org}.opendata-home.{width}", org, f"GET {url} (browser)",
                             "the Organization's open data home names no other's host",
                             f"leaked {leaked}", not leaked)
                    ctx.close()
            for key, p in self.people.items():
                org = key.rsplit("-", 1)[0] if key != "platform" else None
                base = f"{scheme}://{self.hosts[self.home(key)]['portal']}" if self.hosts else self.base
                own = sorted(self.exp[org]["projects"])[0] if org else "helsinki"
                foreign = sorted(sorted(o["projects"])[0] for k, o in self.exp.items() if k != org)[0] if org else None
                pages = [("own-project", f"/projects/{own}"), ("projects", "/"), ("organization", "/organization")]
                if foreign:
                    pages.append(("foreign-project", f"/projects/{foreign}"))
                for width, height in ((1440, 900), (400, 860)):
                    ctx = browser.new_context(viewport={"width": width, "height": height})
                    page = ctx.new_page()
                    page.goto(base + "/api/v1/auth/login")
                    page.fill("input[name=username]", p["username"])
                    page.fill("input[name=password]", p["password"])
                    page.locator("input[name=password]").press("Enter")
                    page.wait_for_url(re.compile(re.escape(base) + r"/(?!id/).*"), timeout=30000)
                    for name, path in pages:
                        page.goto(base + path, wait_until="networkidle", timeout=60000)
                        page.wait_for_timeout(1500)
                        file = out / f"{key}-{name}-{width}.png"
                        page.screenshot(path=str(file), full_page=False)
                        text = page.locator("body").inner_text(timeout=5000)
                        shots.append({"persona": key, "page": name, "path": path, "width": width, "file": file.name})
                        if name == "own-project":
                            # The control: the app rendered and names the project, or no other page
                            # of this context is judged.
                            item = next((i for i in (self.session(key).json("GET", "/api/v1/projects")[1] or {})
                                         .get("items", []) if i.get("name") == own), {})
                            title = item.get("title")
                            names = [own, *(title.values() if isinstance(title, dict) else [title] if title else [])]
                            rendered = "UI bundle not built" not in text and any(n.lower() in text.lower() for n in names)
                            self.row(f"ui.{key}.own-project.{width}", org or "all", f"GET {path} (browser)",
                                     f"the Portal app names {own} or its title", text.strip()[:80].replace("\n", " "), rendered)
                            if not rendered:
                                for later, _ in pages[1:]:
                                    if org and later in ("projects", "foreign-project"):
                                        self.row(f"ui.{key}.{later}.{width}", org or "all", "(browser)", "-", "-", None,
                                                 "the Portal app did not render (own-project control): nothing to judge")
                                break
                        if name == "projects" and org:
                            names = set(re.findall(r"[a-z0-9-]+", text.lower()))
                            seen_foreign = sorted(x for x in everything - set(self.exp[org]["projects"])
                                                  if x in names and x not in self.exp[org]["projects"])
                            self.row(f"ui.{key}.projects.{width}", org, f"GET {path} (browser)",
                                     "no project of another org on the page", f"foreign {seen_foreign}",
                                     not seen_foreign)
                        if name == "foreign-project":
                            other = next(k for k, o in self.exp.items() if foreign in o["projects"])
                            leaked = [n for n in self.exp[other]["projects"][foreign]["Endpoint"] if len(n) >= 8 and n != foreign and n in text]
                            self.row(f"ui.{key}.foreign-project.{width}", f"{org}->{other}", f"GET {path} (browser)",
                                     "a not-found page, no object of the project", f"leaked {leaked[:3]}", not leaked)
                    ctx.close()
            browser.close()
        (out / "shots.json").write_text(json.dumps(shots, indent=1))

    def run(self):
        manifest = json.loads((self.export / "manifest.json").read_text())
        if "multiorg" not in manifest:
            self.row("setup", "all", "read manifest.json", "a multiorg block (compose.py)", "none", None,
                     "not applicable: the export is one installation, not a composed multi-organization bundle")
            return
        self.expectations()
        if not self.state:
            self.row("setup", "all", "-", "--state", "none", None, "no --state: persons cannot be made")
            return
        try:
            self.setup()
        except Exception as ex:  # noqa: BLE001 - a setup failure is a row, the rest is skipped
            self.row("setup", "all", "persons and bindings", "made", str(ex)[:300], False)
            return
        steps = (self.check_projects, self.check_hosts, self.check_project_reads, self.check_writes, self.check_people,
                 self.check_mcp, self.check_connectors, self.check_data, self.check_catalogue, self.check_dashboards, self.check_global)
        if self.a.shots:
            steps += (self.check_ui,)
        for step in steps:
            if self.a.only and step.__name__.removeprefix("check_") not in self.a.only.split(","):
                continue
            try:
                step()
            except Exception as ex:  # noqa: BLE001
                self.row(f"{step.__name__}.crashed", "all", step.__name__, "runs", repr(ex)[:300], False)

    def write(self, out: Path):
        out.mkdir(parents=True, exist_ok=True)
        (out / "results.json").write_text(json.dumps({"suite": "multiorg", "base": self.base, "checks": self.rows},
                                                     indent=1))
        tools = {k: {"portal": v.get("tools"), "connector": v.get("connectorTools")}
                 for k, v in self.people.items() if v.get("tools") is not None or v.get("connectorTools")}
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
    ap.add_argument("--only", help="comma list of check groups: projects,hosts,project_reads,writes,people,mcp,connectors,data,catalogue,dashboards,global,ui")
    ap.add_argument("--shots", action="store_true", help="browser shots 1440/400 per persona (Playwright)")
    ap.add_argument("--sync-timeout", type=int, default=180)
    ap.add_argument("--jc-config", "--hosts", dest="jc_config",
                    help="the jc.toml (or its [[organizations]] fragment): host mode, each Organization on its own hosts")
    a = ap.parse_args(argv)
    R.loopback_only(a.base_url)
    s = Suite(a)
    s.run()
    n = s.write(a.out)
    print(json.dumps(n))
    return 1 if n["fail"] else 0
