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
404 and A's own prefixed form answers in place (API) or 308 (page).

Counting (GOAL-multiorg): every ordered pair of Organizations (A acting on B) has at least one
isolation row, or `setup.pairs` fails; the skips of each row template (the id with Organization and
project names as {o}, object names as {n}) stay within skip-bound.json, measured on the
three-Organization bundle; both are printed in the run line.

Live mode (--live, T-6851; without it the base must be loopback): the base is one of LIVE_HOSTS
(https) or loopback (a rehearsal of the live mode). Every request passes `Allowlist` first and is
logged in <out>/requests.log: GET anywhere; POST to the realm's token endpoint and sign-in form,
NGSI-LD entityOperations/query, MCP initialize and tools/list, and tools/call of a tool whose
annotations say readOnlyHint; any other method or path only on a project of ctl-a or ctl-b, or the
DELETE of the run's own personas. A request outside is refused before it is sent. The personas
come from --personas (the file the owner-approved Operator Change made; `--make-personas FILE`
writes it on a local jc); no ui, connectors or operators rows; after the run, also a failed one,
the personas are deleted (--state on a local jc) or listed in <out>/cleanup.json for the
operator's cleanup Change, and the credentials file is removed."""
import argparse
import base64
import hashlib
import ipaddress
import json
import os
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
LIVE_HOSTS = ("jc.jcdev.dev.joinedcontext.com", "smartcity.joinedcontext.com")
FIXTURES = ("ctl-a", "ctl-b")  # the Organizations whose projects a live run may write into
LIVE_OFF = ("ui", "connectors", "operators", "portals")  # a browser, or writes outside the fixtures
PORTAL_PATH = "multiorg"  # the further Portal setup commits for each Organization (check_portals)
BOUND = Path(__file__).resolve().parent / "skip-bound.json"
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


class Refused(RuntimeError):
    """A request the live allowlist does not admit; raised before anything is sent."""


def live_base(base: str) -> None:
    """--live: one of the live deployments over https, or loopback (a rehearsal of the live mode)."""
    u = urllib.parse.urlparse(base)
    if (u.hostname or "").lower() in LIVE_HOSTS and u.scheme == "https" and u.port in (None, 443):
        return
    host = u.hostname or ""
    if host != "localhost":
        try:
            if ipaddress.ip_address(host).is_loopback:
                return
        except ValueError:
            pass
        raise SystemExit(f"--live: {host or base} is none of {', '.join(LIVE_HOSTS)} (https) and not loopback")


class Allowlist:
    """What a --live run may send (module docstring); `check` logs each request and refuses one
    outside before it leaves. `fixtures` are the projects of ctl-a/ctl-b, `personas` the run's own
    user ids, `hosts` the addresses of the deployment."""

    SIGN_IN = re.compile(r"/id/realms/[\w.-]+/(?:protocol/openid-connect/token|login-actions/authenticate)")
    MCP = re.compile(r"/api/v1/mcp|/c/[\w-]+|/api/endpoint/[a-z2-7]+/mcp")
    MCP_READS = ("initialize", "notifications/initialized", "tools/list", "ping")

    def __init__(self, log: Path, hosts, fixtures=(), personas=()):
        self.log, self.hosts = log, set(hosts)
        self.fixtures, self.personas = set(fixtures), set(personas)
        self.read_only: set[str] = set()

    def verdict(self, method: str, url: str, body) -> str | None:
        u = urllib.parse.urlparse(url)
        if u.netloc.lower() not in self.hosts:
            return None
        path = u.path
        if method in ("GET", "HEAD"):
            return "read"
        project = re.match(r"/api/v1/projects/([\w.-]+)(?:/|$)", path)
        if project and project.group(1) in self.fixtures:
            return "fixture"
        person = re.fullmatch(r"/id/admin/realms/[\w.-]+/users/([\w-]+)", path)
        if method == "DELETE" and person and person.group(1) in self.personas:
            return "cleanup"
        if method != "POST":
            return None
        if self.SIGN_IN.fullmatch(path):
            return "sign-in"
        if path.endswith("/ngsi-ld/v1/entityOperations/query"):
            return "query"
        if self.MCP.fullmatch(path):
            try:
                rpc = json.loads(body) if isinstance(body, bytes) else body
            except ValueError:
                return None
            method_ = (rpc or {}).get("method") if isinstance(rpc, dict) else None
            if method_ in self.MCP_READS:
                return "mcp"
            if method_ == "tools/call" and ((rpc.get("params") or {}).get("name") in self.read_only):
                return "mcp read-only tool"
        return None

    def check(self, method: str, url: str, body) -> None:
        why = self.verdict(method, url, body)
        u = urllib.parse.urlparse(url)
        with self.log.open("a") as f:  # the path without its query: no code, state or search term
            f.write(f"{method} {u.netloc}{u.path} {why or 'REFUSED'}\n")
        if why is None:
            raise Refused(f"--live: {method} {u.path} is outside the allowlist")

    def learn(self, answer) -> None:
        """The read-only tools of a tools/list answer (annotations.readOnlyHint)."""
        tools = ((answer or {}).get("result") or {}).get("tools", []) if isinstance(answer, dict) else []
        self.read_only |= {t["name"] for t in tools if (t.get("annotations") or {}).get("readOnlyHint") is True}

    def guard(self, h: "R.Http") -> "R.Http":
        """`h` with every call checked first; MCP tool lists teach the read-only tools."""
        inner = h.call

        def call(method, path, body=None, headers=None, form=None, timeout=60):
            self.check(method, h.url(path), body)  # the address the request names (HostHttp: its host)
            st, hdrs, raw = inner(method, path, body, headers, form, timeout)
            if isinstance(body, dict) and body.get("method") == "tools/list" and st == 200:
                try:
                    self.learn(json.loads(raw))
                except ValueError:
                    pass
            return st, hdrs, raw

        h.call = call
        return h


def portal_manifest(name: str, org: str, sources: list, path: str = PORTAL_PATH) -> str:
    """A Portal manifest (ODP-08) of Organization `org` at `/{org}/opendata/-/{path}/`."""
    return json.dumps({"apiVersion": "joinedcontext.com/v1alpha1", "kind": "Portal",
                       "metadata": {"name": name, "namespace": "org"},
                       "spec": {"organizationRef": org, "path": path, "title": {"en": f"multiorg {org}"},
                                "sources": sources}}, indent=1) + "\n"


def users(emails) -> list[str]:
    """The user names of e-mail addresses, sorted: rows name people by user name, never by address
    (the logs suite fails any result file naming an e-mail address)."""
    return sorted(e.partition("@")[0] for e in emails)


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
        self.installation: str | None = None
        self.export = Path(a.export)
        self.people: dict[str, dict] = {}
        self.dashboards: dict[str, tuple[str, str]] = {}
        self.exchange_ok = False  # an own control exchange succeeded: a foreign refusal then counts  # org -> (project, name) the suite committed
        self.hosts = organization_hosts(Path(a.jc_config)) if a.jc_config else {}
        self.live = bool(getattr(a, "live", False))
        self.guard: Allowlist | None = None
        if self.live:
            base = urllib.parse.urlparse(self.base).netloc.lower()
            self.guard = Allowlist(Path(a.out) / "requests.log", {base, *(h for o in self.hosts.values() for h in o.values())})

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
        history_of: dict[str, set] = {}  # tenant -> the entity types the bundle holds history of
        temporal = self.export / "antares" / "temporal.jsonl"
        for line in temporal.open() if temporal.is_file() else ():
            row = json.loads(line)
            history_of.setdefault(row["tenant"], set()).add(row["type"])
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
                # space -> the types a Policy there grants the role `public` retrieveTemporal of and
                # the bundle holds history of (the tenant is the space), and space -> its public
                # Endpoints (name, slug): a public Endpoint maps its caller to that role
                history, endpoints = {}, {}
                for q in (files / CONFIG[0] / p).glob("spaces/*/policies/*.yaml"):
                    text = q.read_text(errors="replace")
                    public = re.search(r"assignee:\s*\{[^}]*\bid:\s*public\b|^\s*id:\s*public\s*$", text, re.M)
                    if public and re.search(r"retrieveTemporal", text):
                        kinds_q = re.findall(r"^\s*-\s*type:\s*([\w.-]+)", text, re.M)
                        held = history_of.get(q.parent.parent.name, set())
                        history.setdefault(q.parent.parent.name, set()).update(k for k in kinds_q if k in held)
                closed = {}  # slug -> (name, audience) of every Endpoint that is not public
                readable = []  # of them, those whose space has a Policy granting a read to anyone
                for f in (files / CONFIG[0] / p).glob("spaces/*/endpoints/*.yaml"):
                    text = f.read_text(errors="replace")
                    aud = re.search(r"^\s*audience:\s*([\w-]+)", text, re.M)
                    slug = re.search(r"^\s*slug:\s*([a-z2-7]+)", text, re.M)
                    name = re.search(r"^metadata:\s*\n(?:\s+.*\n)*?\s+name:\s*([\w.-]+)", text, re.M)
                    if slug and name and aud and aud.group(1) == "public":
                        endpoints.setdefault(f.parent.parent.name, []).append((name.group(1), slug.group(1)))
                    if aud and slug and name and aud.group(1) != "public":
                        closed[slug.group(1)] = (name.group(1), aud.group(1))
                        if any(re.search(r"^\s*-\s*retrieveOps\s*$", q.read_text(errors="replace"), re.M)
                               for q in (f.parent.parent / "policies").glob("*.yaml")):
                            readable.append(slug.group(1))
                ckan = []
                for f in (files / CONFIG[0] / p / "ckan").glob("*.yaml"):
                    d = re.search(r"organizationDefault:\s*([\w.-]+)", f.read_text())
                    if d:
                        ckan.append(d.group(1))
                spaces = sorted(f.parent.name for f in (files / CONFIG[0] / p).glob("spaces/*/space.yaml"))
                exp[org]["projects"][p] = {k: sorted(set(v)) for k, v in kinds.items()} | {"ckanOrgs": ckan,
                                                                                          "closed": closed,
                                                                                          "readable": readable,
                                                                                          "spaces": spaces,
                                                                                          "history": {sp: (sorted(t), sorted(endpoints.get(sp, [])))
                                                                                                      for sp, t in sorted(history.items()) if t and endpoints.get(sp)}}
        self.exp = exp
        orgs = files / CONFIG[0] / CONFIG[1] / "organizations"
        self.installation = next((f.stem for f in orgs.glob("*.yaml")
                                  if re.search(r"^\s*installation:\s*true", f.read_text(), re.M)), None)
        return exp

    # ---- personas
    def client(self, org: str | None = None, kind: str = "portal") -> R.Http:
        """Path mode: the base. Host mode: `org`'s `kind` host (the base without an org)."""
        if not self.hosts:
            h = R.Http(self.base)
        else:
            every = [h for o in self.hosts.values() for h in o.values()]
            h = HostHttp(self.base, self.hosts[org][kind] if org else None, every)
        return self.guard.guard(h) if self.guard else h

    def home(self, key: str) -> str | None:
        """The Organization whose Portal host persona `key` works on in host mode: its own; the
        platform admin the installation's."""
        if not self.hosts:
            return None
        return self.installation if key == "platform" else key.rsplit("-", 1)[0]

    def personas_from(self, file: Path) -> None:
        """--personas: the run's people as the owner-approved Operator Change made them, in a file
        only its owner reads (0600, in a 0700 directory)."""
        for f, mode in ((file, 0o600), (file.parent, 0o700)):
            if f.stat().st_mode & 0o777 & ~mode:
                raise RuntimeError(f"{f.name} is readable by others (mode {f.stat().st_mode & 0o777:o}); want {mode:o}")
        people = json.loads(file.read_text())
        want = {"platform", *(f"{o}-{k}" for o in self.exp for k in ("admin", "member"))}
        if set(people) != want:
            raise RuntimeError(f"--personas names {sorted(set(people) ^ want)[:6]} unlike the bundle's Organizations")
        self.people = {k: {"username": v["username"], "id": v["id"], "password": v["password"]} for k, v in people.items()}

    def save_personas(self, file: Path) -> None:
        """--make-personas: the people setup made, for a later --live run (the local stand-in for the
        Operator Change), 0600 in a 0700 directory."""
        file.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(file.parent, 0o700)
        fd = os.open(file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({k: {"username": v["username"], "id": v["id"], "password": v["password"]}
                       for k, v in self.people.items()}, f)

    def cleanup(self) -> None:
        """After a --live run, also a failed one: the personas deleted through the realm admin (a
        local jc's --state), else listed for the operator's cleanup Change; the credentials gone."""
        ids = {k: v["id"] for k, v in self.people.items() if v.get("id")}
        left = dict(ids)
        if self.state and ids:
            self.guard.personas = set(ids.values())
            h = self.client()
            auth = {"Authorization": f"Bearer {R.admin_token(h, self.state)}"}
            for key, uid in ids.items():
                st, _, _ = h.call("DELETE", f"/id/admin/realms/{self.realm}/users/{uid}", headers=auth)
                if st in (204, 404):
                    left.pop(key)
        (Path(self.a.out) / "cleanup.json").write_text(json.dumps(
            {"personas": sorted(left.values()), "deleted": len(ids) - len(left)}, indent=1))
        self.row("live.cleanup", "all", "DELETE each persona", "none left", f"{len(left)} left", not left,
                 "" if not left else "the operator's cleanup Change removes those in cleanup.json")
        if self.a.personas:
            Path(self.a.personas).unlink(missing_ok=True)
        for p in self.people.values():
            p.pop("password", None)

    def setup(self):
        if self.live:
            self.personas_from(Path(self.a.personas))
            self.guard.fixtures = {p for o in FIXTURES for p in self.exp.get(o, {}).get("projects", {})}
            self.guard.personas = {v["id"] for v in self.people.values()}
            return self.synced()
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
            if not e["projects"]:  # a DelegationPolicy names projects: an empty control has none
                continue
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
        # One further Portal per Organization listing its published Endpoints (ODP-10), read by
        # check_portals: a forge commit as the RoleBindings, as an approved Change would be. The
        # bundle's org-admin Role predates the kind: it gains Portal, as a deployment's does.
        portals = [(f"portals/multiorg-{org}.yaml", portal_manifest(f"multiorg-{org}", org, [
            {"name": "published", "type": "endpoints"}])) for org in self.exp]
        role = self.read_file("users/roles/org-admin.yaml")
        if role and not re.search(r"^\s*- Portal\s*$", role, re.M):
            role = re.sub(r"^(\s*)- Organization\s*$", r"\g<0>\n\1- Portal", role, count=1, flags=re.M)
            portals.append(("users/roles/org-admin.yaml", role))
        self.commit(portals, "multiorg smoke: a further Portal of each Organization")
        self.synced()

    def synced(self):
        """Waits until each admin sees its Organization's projects (the Portal's sync)."""
        deadline = time.time() + self.a.sync_timeout
        pending = set(self.exp)
        why: dict[str, str] = {}
        while pending and time.time() < deadline:
            for org in sorted(pending):
                st, body = self.session(f"{org}-admin").json("GET", "/api/v1/projects")
                st_d = self.session("platform").json("GET", f"/api/v1/projects/org/delegationpolicies/multiorg-{org}")[0] \
                    if self.exp[org]["projects"] and not self.live else 200
                dash = self.dashboards.get(org)
                st_b = self.session("platform", org if self.hosts else None).json("GET", f"/api/v1/projects/{dash[0]}/dashboards/{dash[1]}")[0] if dash else 200
                if st == 200 and st_d == 200 and st_b == 200 and \
                        (not self.state or not self.exp[org]["projects"]  # {org}_bound: Organizations with a project
                         or self.bound(org) >= {f"mo-{org}-admin", f"mo-{org}-member"}) and \
                        set(self.exp[org]["projects"]) <= {i["name"] for i in body.get("items", [])}:
                    pending.discard(org)
                else:
                    why[org] = f"projects {st}, delegation {st_d}, dashboard {st_b}" + \
                        (f", bound {sorted(self.bound(org))}" if self.state else "")
            if pending:
                time.sleep(5)
        # Sessions and tokens taken while waiting predate the {org}_bound groups (jc_org): every
        # check signs in again.
        for p in self.people.values():
            p.pop("http", None)
            p.pop("tokens", None)
        self.row("setup.bindings-synced", ",".join(self.exp), "GET /api/v1/projects (each org admin)",
                 "every admin lists its projects; DelegationPolicy, Dashboard and {org}_bound group synced",
                 f"not yet: {[f'{o} ({why.get(o)})' for o in sorted(pending)]}" if pending else "all",
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

    def read_file(self, path, repo=CONFIG) -> str | None:
        """A file of `main` in the forge, or None."""
        auth = {"Authorization": f"token {R.read_secret(self.state, 'shako-token')}"}
        st, cur = self.client().json("GET", f"/git/api/v1/repos/{repo[0]}/{repo[1]}/contents/{path}?ref=main", headers=auth)
        return base64.b64decode(cur.get("content", "")).decode() if st == 200 and isinstance(cur, dict) else None

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
                # the audit feed of each space (temporal-audit), as the activity feed is above
                calls += [f"/api/v1/projects/{project}/spaces/{n}/temporal-audit" for n in objs.get("spaces", [])[:2]]
                # the export of named manifests stays with the project's readers (a whole project is
                # the installation administrators', UI-87)
                calls += [f"/api/v1/projects/{project}/export?names={n}" for n in objs["Endpoint"][:1]]
                for call in calls:
                    st_own, _, _ = self.session(f"{owner}-admin").call("GET", call)
                    if st_own != 200:  # a read the Organization's admin lacks: the operator's 200 shows the route
                        st_own, _, _ = self.session("platform", owner if self.hosts else None).call("GET", call)
                    for other in self.exp:
                        if other == owner:
                            continue
                        for kind in ("admin", "member"):
                            st, _, body = self.session(f"{other}-{kind}").call("GET", call)
                            rid = f"isolation.{other}-{kind}.{project}{call.split(project, 1)[1] or '/'}"
                            if st_own != 200:
                                self.row(rid, f"{other}->{owner}", f"GET {call}", "control 200, foreign 403/404",
                                         f"control {st_own}, foreign {st}", st in DENIED and None,
                                         f"no control: {owner}-admin and the platform admin got {st_own}" if st in DENIED
                                         else f"foreign got {st}, control {st_own}")
                                if st not in DENIED:
                                    self.rows[-1]["result"] = "fail"
                                continue
                            # the refusal's body names nothing of the owner beyond what the call asked for
                            text = body.decode(errors="replace")
                            named = sorted(m for m, o in self.marks_of(other).items()
                                           if o == owner and m.strip('"') not in call and m in text)
                            self.row(rid, f"{other}->{owner}", f"GET {call}", "403/404 naming nothing of it (control 200)",
                                     f"{st} {body[:120]!r}" if st not in DENIED or named else str(st) + f", names {named[:3]}",
                                     st in DENIED and not named)
                    if "/export?names=" in call:  # the own export: the named Endpoint, nothing of another
                        st_e, _, raw = self.session(f"{owner}-admin").call("GET", call)
                        text = raw.decode(errors="replace")
                        named = sorted(m for m, o in self.marks_of(owner).items() if m in text)
                        name = call.rsplit("=", 1)[1]
                        self.row(f"export.{owner}-admin.{project}", owner, f"GET {call} as mo-{owner}-admin",
                                 f"200 holding {name}, nothing of another Organization",
                                 f"{st_e}, {len(raw)} bytes, foreign {named[:3]}",
                                 st_e == 200 and f"name: {name}" in text and not named)

    def check_writes(self):
        """A foreign org admin's DELETE of an Endpoint is refused and the Endpoint stays."""
        for owner, e in self.exp.items():
            project = next((p for p, o in e["projects"].items() if o["Endpoint"]), None)
            if not project or (self.live and owner not in FIXTURES):
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
            foreign = users(x for x in emails if (m := re.match(r"mo-(.+)-(admin|member)@", x)) and m.group(1) != org)
            # The list is realm-wide (people.rs list_people asks read on Person at the installation):
            # an admin of another Organization is refused it and has no list of its own members.
            gap = org != self.installation and st in DENIED
            self.row(f"people.{org}-admin.own", org, "GET /api/v1/organization/people", f"{users(own)} listed",
                     f"{st}, missing {users(own - emails)}", None if gap else (st == 200 and own <= emails),
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

    def marks_of(self, org):
        """foreign_marks(org), once per run."""
        held = self.__dict__.setdefault("_marks", {})
        if org not in held:
            held[org] = self.foreign_marks(org)
        return held[org]

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

        own = next((p for p, o in sorted(self.exp[org]["projects"].items()) if o["Endpoint"]), None)
        if own and not (self.live and "jc_resource_list" not in self.guard.read_only):
            st, res = call("jc_resource_list", {"project": own, "kind": "Endpoint"})
            text = json.dumps(res, ensure_ascii=False)
            missing = [n for n in self.exp[org]["projects"][own]["Endpoint"] if f'\\"name\\":\\"{n}\\"' not in text]
            self.row(f"mcp.{key}.own-endpoints.{own}", org, f"tools/call jc_resource_list {own}",
                     f"every Endpoint of {own} ({len(self.exp[org]['projects'][own]['Endpoint'])})",
                     f"{st}, missing {missing[:4]}", st == 200 and not missing)
        # An Organization with no project names another's: refused, or an answer naming nothing of
        # it; the operators' Organization's admin is judged by the operators rows.
        probe = own or next((min(oe["projects"]) for o, oe in self.exp.items() if o != org and oe["projects"]), None)
        operator = not own and key == f"{self.installation}-admin"
        for name in ("jc_endpoint_list_all", "jc_person_list", "jc_member_list", "jc_organization_read",
                     "jc_catalog_search", "jc_permissions_me", "jc_workspace_list", "jc_activity_list"):
            if operator or name not in self.people[key].get("tools", []) or (self.live and name not in self.guard.read_only):
                continue
            args = ({"project": probe} if probe else {}) | {"jc_catalog_search": {"q": "air"},
                                       "jc_organization_read": {"aspect": "projects"}}.get(name, {})
            st, res = call(name, args)
            r = (res or {}).get("result") or {}
            text = json.dumps(res, ensure_ascii=False)
            found = leaks(text, skip=(probe,) if probe != own else ())
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
            project = min(oe["projects"], default=None)
            for name, args in (("jc_resource_list", {"project": project, "kind": "Endpoint"}),
                               ("jc_project_get", {"project": project}),
                               ("jc_change_list", {"project": project}),
                               ("jc_organization_read", {"organization": other})):
                if name not in self.people[key].get("tools", []) or (project is None and "project" in args) \
                        or (self.live and name not in self.guard.read_only):
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
            if not e["projects"]:  # a connector is scoped to a project: an empty control makes none
                continue
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
                if other == org or not oe["projects"]:
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
            # Organization Endpoints a person can read at all: a space without a read Policy lets nobody
            # read its Endpoints, so the readable catalogue rightly lists none of them.
            org_aud = {slug for p, pe in self.exp[org]["projects"].items()
                       for slug, (_, aud) in pe["closed"].items() if aud == "organization"}
            own_org = {slug for pe in self.exp[org]["projects"].values() for slug in pe["readable"]} & org_aud
            for kind in ("admin", "member"):
                key = f"{org}-{kind}"
                st, body = self.session(key).json("GET", "/api/v1/catalogue/readable")
                slugs = {d.get("slug") for d in (body or {}).get("datasets", [])} if st == 200 else set()
                seen = sorted(f"{foreign[x][0]}/{foreign[x][1]}" for x in slugs & set(foreign))
                self.row(f"data.{key}.readable.no-foreign", org, "GET /api/v1/catalogue/readable",
                         "no closed Endpoint of another org", f"{st}, {len(slugs)} listed, foreign {seen[:4]}",
                         st == 200 and not seen)
                if org_aud and not own_org:
                    self.row(f"data.{key}.readable.own-organization", org, "GET /api/v1/catalogue/readable",
                             f">= 1 of its {len(org_aud)} organization Endpoints", f"{st}", None,
                             "no organization Endpoint of it has a read Policy in its space: nobody may read one")
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
                    if tok and code == 200:
                        self.check_temporal(key, org, tok, foreign)
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

    def check_temporal(self, key, org, tok, foreign):
        """History through the gateway (DAT-19): the own Organization's public Endpoint of a space
        whose Policy grants the role `public` retrieveTemporal of a type the bundle holds history
        of answers that type's temporal and aggregated reads (present, the control); the person's
        own Endpoint token on another Organization's closed Endpoints answers nothing (absent)."""
        held = next(((p, sp, v) for p, pe in sorted(self.exp[org]["projects"].items())
                     for sp, v in pe.get("history", {}).items()), None)
        if not held:
            return
        project, space, (kinds, public) = held
        api = self.client(self.home(key), "api")
        kind = urllib.parse.quote(kinds[0])
        since = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 7 * 86400))
        # max: a numeric attribute's history aggregates; totalCount and sum are not meaningful
        # over a property recorded only on change (Antares answers 400 aggregation-not-meaningful)
        reads = {"temporal": f"/temporal/entities?type={kind}&timerel=after&timeAt={since}&lastN=1",
                 "aggregated": f"/temporal/entities?type={kind}&timerel=after&timeAt={since}&aggrMethods=max"
                               f"&aggrPeriodDuration=P1D&format=aggregatedValues"}
        # the space's public Endpoint that holds the type (an App's Endpoint may hold other types)
        endpoint, slug = public[0]
        for endpoint, slug in public[:8]:
            st, body = api.json("GET", f"/api/endpoint/{slug}/ngsi-ld/v1{reads['temporal']}")
            if st == 200 and isinstance(body, list) and body:
                break
        for name, q in reads.items():
            base = f"/api/endpoint/{slug}/ngsi-ld/v1"
            st, body = api.json("GET", base + q)
            n = len(body) if isinstance(body, list) else 0
            self.row(f"data.{key}.{name}.own", org, f"GET {base}{q} ({project}/{endpoint}, public)",
                     f"200 with >= 1 {kinds[0]} (a Policy of {space} grants the role public retrieveTemporal)",
                     f"{st}, {n} entities" + ("" if st == 200 else f" {str(body)[:300]}"), st == 200 and n > 0)
            for fslug, (p, ename, _) in sorted(foreign.items())[:6]:
                other = next(o for o, oe in self.exp.items() if p in oe["projects"])
                st, raw = api.json("GET", f"/api/endpoint/{fslug}/ngsi-ld/v1{q}", headers={"Authorization": f"Bearer {tok}"})
                self.row(f"data.{key}.{name}.{p}/{ename}", f"{org}->{other}",
                         f"GET /api/endpoint/{fslug}/ngsi-ld/v1{q} with the own Endpoint's token",
                         "401/403/404 (the own read: 200)", f"{st} {str(raw)[:80]}" if st not in (401, *DENIED) else str(st),
                         st in (401, *DENIED))

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
                                   ("dcat", f"/opendata/publisher/{c}/catalog.jsonld"),
                                   ("ttl", f"/opendata/publisher/{c}/catalog.ttl"),
                                   ("atom", f"/opendata/feeds/organization/{c}.atom")):
                    st, _, raw = h.call("GET", path)
                    text = raw.decode(errors="replace")
                    other_names = {r.get("name") for r in results
                                   if (r.get("organization") or {}).get("name") in others}
                    leaked = sorted(n for n in other_names if n and re.search(rf"dataset/{re.escape(n)}(?![\w-])", text))
                    empty = st == 404 and not names and not leaked
                    # the positive control: an own dataset is in the feed when the search lists one
                    own = not names or any(re.search(rf"dataset/{re.escape(n)}(?![\w-])", text) for n in names if n)
                    self.row(f"catalogue.{org}.{c}.{kind}", org, f"GET {path}", "200, an own dataset, no dataset of another org",
                             f"{st}, {len(names)} own, own in it {own}, leaked {leaked[:3]}",
                             None if empty else (st == 200 and own and not leaked),
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
                               ("dcat", "/catalog.jsonld"), ("ttl", "/catalog.ttl"), ("atom", "/feeds/dataset.atom"),
                               ("sitemap", "/sitemap.xml"), ("home", "/")):
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
                # the positive control of a feed: one of its own datasets is in it
                own = kind not in ("dcat", "ttl", "atom") or not found[org] or \
                    any(re.search(rf"/dataset/{re.escape(n)}(?![\w-])", text) for n in found[org] if n)
                self.row(f"catalogue.{org}.{kind}.no-foreign", org, f"GET {{opendata}}{path}",
                         "200, no dataset or host of another org (a feed: an own dataset in it)",
                         f"{st}, own in it {own}, leaked {leaked[:3]}, hosts {hosts}",
                         st == 200 and own and not leaked and not hosts)

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
                foreign = min(self.exp[other]["projects"], default=None)
                if foreign:
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
                st_w = None if self.live else replay.call(  # live: the read alone (no write outside the fixtures)
                    "POST", f"/api/v1/projects/{min(self.exp[org]['projects'], default='org')}/workspaces",
                    headers={"Cookie": cookie, "x-csrf-token": jar.get("__Host-jc_csrf", ("",))[0]}, body={})[0]
                self.row(f"hosts.cookie.{other}.on.{org}", f"{other}->{org}",
                         f"GET /api/v1/projects, POST workspaces on {org}'s host with {other}'s cookies",
                         "401 / 401-403 (control 200 on its own host)", f"read {st_r}, write {st_w}, control {st_own}",
                         st_own == 200 and st_r == 401 and (self.live or st_w in (401, 403)))
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

    def check_forged(self):
        """A forged Host or X-Forwarded-Host never switches the Organization (I-7, I-8): A's session
        sent with B's host answers what it answers plainly (the control, 200) or is refused, never
        anything of B; a token's iss and aud never follow a forged host. Host mode: the forged
        X-Forwarded-Host on A's own host (A's cookies on B's host are `hosts.cookie`). Path mode:
        one domain, so a forged Host as well, with A's cookies."""
        everything = {p for o in self.exp.values() for p in o["projects"]}
        forged = {o: self.hosts[o]["portal"] if self.hosts else f"portal.{o}.localhost" for o in self.exp}

        def projects(h, headers=None):
            st, body = h.json("GET", "/api/v1/projects", headers=headers)
            return st, sorted({i["name"] for i in (body or {}).get("items", [])} if st == 200 else ())

        for a in self.exp:
            h = self.session(f"{a}-admin")
            st_c, control = projects(h)
            mine = set(self.exp[a]["projects"])
            ok_c = st_c == 200 and mine <= set(control)
            self.row(f"forged.{a}-admin.control", a, "GET /api/v1/projects (no forged header)",
                     f"200 listing {sorted(mine)}", f"{st_c}, {control[:6]}", ok_c)
            for b in self.exp:
                if b == a:
                    continue
                forms = {"xfh": {"X-Forwarded-Host": forged[b]}}
                if not self.hosts:
                    forms["host"] = {"Host": forged[b]}
                for form, headers in forms.items():
                    st, seen = projects(h, headers)
                    theirs = sorted((set(seen) - set(control)) & everything)
                    self.row(f"forged.{a}-admin.{form}.{b}", f"{a}->{b}",
                             f"GET /api/v1/projects with {next(iter(headers))}: {forged[b]}",
                             "the control's answer, or 401/403/404/421; nothing of another Organization",
                             f"{st}, {len(seen)} projects, beyond the control {theirs[:4]}",
                             (st in (401, 403, 404, 421) or (st == 200 and seen == control)) if ok_c else None,
                             "" if ok_c else "no control: the plain call failed")
        # kaisatsu builds issuer and audience from the request's host (I-7): a forged one must not
        # reach a discovery document or a token.
        h = self.client()
        well_known = f"/id/realms/{self.realm}/.well-known/openid-configuration"
        st0, plain = h.json("GET", well_known)
        issuer = (plain or {}).get("issuer") if st0 == 200 else None
        token_call = f"/id/realms/{self.realm}/protocol/openid-connect/token"
        grant = {"grant_type": "client_credentials", "client_id": "portal-api",
                 "client_secret": R.read_secret(self.state, "portal-client-secret")} if self.state else None
        st_t, tok = h.json("POST", token_call, form=grant) if grant else (None, None)
        plain_claims = claims(tok["access_token"]) if st_t == 200 and isinstance(tok, dict) else {}
        evil = "portal.mo-forged.localhost"
        forms = {"xfh": {"X-Forwarded-Host": evil}}
        if not self.hosts:
            forms["host"] = {"Host": evil}
        for form, headers in forms.items():
            st, body = h.json("GET", well_known, headers=headers)
            got = (body or {}).get("issuer") if st == 200 else None
            self.row(f"forged.issuer.{form}", "all", f"GET {well_known} with {next(iter(headers))}: {evil}",
                     f"the issuer {issuer}, or a refusal", f"{st} {got}",
                     (st in (400, 404, 421) or got == issuer) if issuer else None,
                     "" if issuer else f"no control: the plain discovery answered {st0}")
            if not grant:
                continue
            st, body = h.json("POST", token_call, form=grant, headers=headers)
            c = claims(body["access_token"]) if st == 200 and isinstance(body, dict) and body.get("access_token") else {}
            self.row(f"forged.token.{form}", "all", f"POST {token_call} (client_credentials) with {next(iter(headers))}: {evil}",
                     f"iss {plain_claims.get('iss')} and aud as without it, or a refusal",
                     f"{st}, iss {c.get('iss')}, aud {c.get('aud')}",
                     (st in (400, 401, 404, 421) or (c.get("iss"), c.get("aud")) == (plain_claims.get("iss"), plain_claims.get("aud")))
                     if plain_claims else None, "" if plain_claims else f"no control: the plain token answered {st_t}")

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
                want = [org] if names_of(org) else []  # an Organization with no Endpoint: none named
                self.row(f"global.platform.{org}./api/v1/endpoints", org,
                         f"GET /api/v1/endpoints as the platform admin on {org}'s host",
                         f"200 naming Endpoints of {want or 'no Organization'} alone" + ("" if home else ", or 404"),
                         f"{st}, orgs {orgs_seen}",
                         (st == 200 and orgs_seen == want) or (not home and st == 404))
        else:
            st, body = self.session("platform").json("GET", "/api/v1/endpoints")
            text = json.dumps(body, ensure_ascii=False)
            orgs_seen = sorted(o for o in self.exp if any(m in text for m in names_of(o)))
            want = sorted(o for o in self.exp if names_of(o))
            self.row("global.platform./api/v1/endpoints", "all", "GET /api/v1/endpoints as the platform admin",
                     f"200 naming Endpoints of every Organization that has one ({want})", f"{st}, orgs {orgs_seen}",
                     st == 200 and orgs_seen == want)
        for org in self.exp:
            marks = self.foreign_marks(org)
            for kind in ("admin", "member"):
                key = f"{org}-{kind}"
                answered = []  # (path, text) of each 200
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
                    elif st == 200:  # the per-pair rows judge what the per-path rows judge
                        answered.append((path, text))
                    self.row(f"global.{key}.{path}", org, f"GET {path} as mo-{key}",
                             "403/404, or 200 naming no object of another Organization",
                             f"{st}, foreign {hits[:4]}", ok, reason)
                # The same answers per Organization: an empty control has no object, its people's
                # names are its marks, so a pair row holds for it too.
                for other in self.exp:
                    if other == org:
                        continue
                    theirs = [m for m, o in marks.items() if o == other]
                    seen = sorted({path for path, text in answered if any(m in text for m in theirs)})
                    self.row(f"global.{key}.none-of.{other}", f"{org}->{other}",
                             f"GET each of {len(self.GLOBAL_READS)} routes without a project as mo-{key}",
                             f"no answer names {other}'s people or objects", f"{len(answered)} answered, named in {seen[:4]}",
                             not seen)

    def check_portals(self):
        """ODP-12: an Organization's Portals, their sources and what they list reach no other
        Organization. Through the Portal API each admin lists and reads its own further Portal
        (setup's `multiorg-{o}`) and none of another's (404), proposes and deletes none of
        another's (403/404; that admin's own dry run 200), and a source of its own Portal that
        names another Organization's project or private Endpoint address is refused (422). In
        host mode each further Portal's address (`{opendata}/-/multiorg/`) lists its own
        published datasets and none of another's, in search, list, DCAT, Turtle, Atom and the MCP
        catalogue tool; path mode serves no Organization catalogue (no [[organizations]]), so
        those rows skip there."""
        tag = f"mo-pt-{secrets.token_hex(3)}"
        own_ok = {}
        for org in self.exp:
            h = self.session(f"{org}-admin")
            st, body = h.json("GET", "/api/v1/projects/org/portals")
            names = {i.get("metadata", {}).get("name") or i.get("name") for i in (body or {}).get("items", [])} if st == 200 else set()
            foreign = sorted(f"multiorg-{o}" for o in self.exp if o != org and f"multiorg-{o}" in names)
            self.row(f"portals.{org}-admin.list", org, "GET /api/v1/projects/org/portals",
                     f"200 with multiorg-{org}, no portal of another Organization",
                     f"{st}, own {f'multiorg-{org}' in names}, foreign {foreign[:3]}",
                     st == 200 and f"multiorg-{org}" in names and not foreign)
            st, _, _ = h.call("GET", f"/api/v1/projects/org/portals/multiorg-{org}")
            own_ok[org] = st == 200
            self.row(f"portals.{org}-admin.read.own", org, f"GET /api/v1/projects/org/portals/multiorg-{org}",
                     "200", str(st), st == 200)
        for org in self.exp:
            h = self.session(f"{org}-admin")
            for other in self.exp:
                if other == org:
                    continue
                call = f"/api/v1/projects/org/portals/multiorg-{other}"
                st, _, raw = h.call("GET", call)
                self.row(f"portals.{org}-admin.read.{other}", f"{org}->{other}", f"GET {call}",
                         "403/404 (its owner reads 200)", f"{st} {raw[:100]!r}" if st not in DENIED else str(st),
                         (st in DENIED) if own_ok[other] else None, "" if own_ok[other] else "no control: the owner's read failed")
                st, _, raw = h.call("DELETE", f"{call}?dryRun=All")
                self.row(f"portals.{org}-admin.delete.{other}", f"{org}->{other}", f"DELETE {call}?dryRun=All",
                         "403/404", f"{st} {raw[:100]!r}" if st not in DENIED else str(st), st in DENIED)
                body = json.loads(portal_manifest(f"{tag}-{other}", other, [], path=tag))
                st, raw = h.json("POST", "/api/v1/projects/org/portals?dryRun=All", body=body)
                st_c, _ = self.session(f"{other}-admin").json("POST", "/api/v1/projects/org/portals?dryRun=All", body=body)
                self.row(f"portals.{org}-admin.propose.{other}", f"{org}->{other}",
                         f"POST /api/v1/projects/org/portals?dryRun=All (a Portal of {other})",
                         "403/404 (its owner's dry run 200)", f"{st} {str(raw)[:100]}, owner {st_c}",
                         (st in DENIED) if st_c == 200 else None, "" if st_c == 200 else f"no control: the owner's dry run answered {st_c}")
                self.check_portal_sources(org, other, tag)
        if not self.hosts:
            for org in self.exp:
                self.row(f"portals.{org}.address", org, f"GET /{org}/opendata/-/{PORTAL_PATH}/", "host mode only",
                         "path mode", None, "path mode serves no Organization catalogue: no [[organizations]]")
            return
        self.check_portal_addresses()

    def check_portal_sources(self, org, other, tag):
        """`org`'s admin proposes its own Portal with a source reaching `other`: an `endpoints`
        source naming `other`'s project, a `dcat-ap` source at `other`'s private Endpoint
        address. Both 422 naming the source; the same Portal with no source dry-runs 200."""
        h = self.session(f"{org}-admin")
        def propose(sources):
            body = json.loads(portal_manifest(f"{tag}-{org}", org, sources, path=tag))
            return h.json("POST", "/api/v1/projects/org/portals?dryRun=All", body=body)
        control, _ = propose([])
        projects = sorted(self.exp[other]["projects"])
        closed = sorted(s for p in self.exp[other]["projects"].values() for s in p["closed"])
        api = self.hosts[other]["api"] if self.hosts else urllib.parse.urlparse(self.base).hostname
        cases = {"foreign-project": [{"name": "theirs", "type": "endpoints", "projects": projects[:1]}] if projects else None,
                 "foreign-endpoint": [{"name": "theirs", "type": "dcat-ap", "url": f"https://{api}/api/endpoint/{closed[0]}/",
                                       "allowedHosts": [api]}] if closed else None}
        for case, sources in cases.items():
            rid = f"portals.{org}-admin.source.{case}.{other}"
            if sources is None:
                self.row(rid, f"{org}->{other}", "POST /api/v1/projects/org/portals?dryRun=All", "422", "-", None,
                         f"{other} has no {'project' if case == 'foreign-project' else 'private Endpoint'}")
                continue
            st, raw = propose(sources)
            text = json.dumps(raw)
            self.row(rid, f"{org}->{other}", f"POST /api/v1/projects/org/portals?dryRun=All ({case} of {other})",
                     "422 naming spec.sources[0] (the Portal without it 200)", f"{st} {text[:140]}, control {control}",
                     (st == 422 and "spec.sources[0]" in text) if control == 200 else None,
                     "" if control == 200 else f"no control: the Portal without sources answered {control}")

    def check_portal_addresses(self):
        """Host mode: each further Portal's address lists its own Organization's published
        datasets (the reconciler copies them within a few rounds) and nothing of another's."""
        found = {}
        for org in self.exp:
            st, body = self.client(org, "opendata").json("GET", "/api/3/action/package_search?rows=1000")
            found[org] = {r.get("name") for r in (body or {}).get("result", {}).get("results", [])} if st == 200 else set()
        base = f"/-/{PORTAL_PATH}"
        listed = {}
        deadline = time.time() + self.a.sync_timeout
        for org in self.exp:
            h = self.client(org, "opendata")
            while True:
                st, body = h.json("GET", f"{base}/api/3/action/package_search?rows=1000")
                listed[org] = {r.get("name") for r in (body or {}).get("result", {}).get("results", [])} if st == 200 else set()
                if (st == 200 and (listed[org] or not found[org])) or time.time() > deadline:
                    break
                time.sleep(5)
            self.row(f"portals.{org}.listed", org, f"GET {{opendata}}{base}/api/3/action/package_search",
                     "200, its own published datasets listed (an Organization that publishes none: none)",
                     f"{st}, {len(listed[org])} of {len(found[org])} published", st == 200 and (bool(listed[org]) or not found[org]))
        for org in self.exp:
            h = self.client(org, "opendata")
            others = {n for o, ns in found.items() if o != org for n in ns} - found[org]
            for kind, path in (("search", "/api/3/action/package_search?rows=1000"), ("list", "/api/3/action/package_list"),
                               ("dcat", "/catalog.jsonld"), ("ttl", "/catalog.ttl"), ("atom", "/feeds/dataset.atom")):
                st, _, raw = h.call("GET", base + path)
                text = raw.decode(errors="replace")
                if kind in ("search", "list"):
                    result = (json.loads(text or "{}") if st == 200 else {}).get("result") or {}
                    names = {r.get("name") for r in result.get("results", [])} if kind == "search" else set(result)
                    leaked = sorted(n for n in others if n in names)
                else:
                    leaked = sorted(n for n in others if n and re.search(rf"/dataset/{re.escape(n)}(?![\w-])", text))
                hosts = sorted({hh for o, hs in self.hosts.items() if o != org for hh in hs.values() if hh in text})
                self.row(f"portals.{org}.{kind}.no-foreign", org, f"GET {{opendata}}{base}{path}",
                         "200, no dataset or host of another Organization", f"{st}, leaked {leaked[:3]}, hosts {hosts}",
                         st == 200 and not leaked and not hosts)
            rpc = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "jc_dataset_search", "arguments": {}}}
            st, body = h.json("POST", f"{base}/mcp", body=rpc)
            text = json.dumps(body, ensure_ascii=False)
            names = {n for n in [*others, *listed[org]] if n and re.search(rf"(?<![\w-]){re.escape(n)}(?![\w-])", text)}
            leaked = sorted(others & names)
            own = not listed[org] or bool(listed[org] & names)
            self.row(f"portals.{org}.mcp.no-foreign", org, f"POST {{opendata}}{base}/mcp tools/call jc_dataset_search",
                     "200, an own dataset, none of another Organization", f"{st}, own {own}, leaked {leaked[:3]}",
                     st == 200 and own and not leaked)

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

    def check_operators(self):
        """The installation's Organization holds no data (PF-121: ops), so it is the operators'
        alone: the platform-admins group and that Organization's admin. They propose an
        Organization, a RoleBinding and a Group at the org namespace (a dry run: no Change is
        filed) and read the pages that fall back to the installation's Organization; no data
        Organization's admin reaches the org namespace. An installation that holds data has no
        operators' Organization: its admin is judged by the per-Organization rows."""
        if self.installation and self.exp[self.installation]["projects"]:
            return
        operators = {"platform": "operators.platform"}
        if self.installation:
            operators[f"{self.installation}-admin"] = f"operators.installation.{self.installation}-admin"
        data = [o for o in self.exp if o != self.installation]
        tag = f"mo-op-{secrets.token_hex(3)}"
        me = self.people["platform"]["id"]
        target = data[0]
        def manifest(kind, spec):
            return {"apiVersion": "joinedcontext.com/v1alpha1", "kind": kind,
                    "metadata": {"name": tag, "namespace": "org"}, "spec": spec}
        bodies = {
            "organizations": manifest("Organization", {"domain": f"{tag}.example", "locales": ["en"], "defaultLocale": "en",
                                                        "projects": {"visibility": "organization", "creation": "org-admin"}}),
            "rolebindings": manifest("RoleBinding", {"subjects": [{"user": me}], "role": "viewer",
                                                     "scope": {"organization": target}}),
            # an Organization's admin binding someone in another Organization (its own are its own)
            "groups": manifest("Group", {"description": "multiorg operator check", "members": [{"user": me}]}),
        }
        for plural, body in bodies.items():
            call = f"/api/v1/projects/org/{plural}?dryRun=All"
            control = False
            for key, rid in operators.items():
                st, raw = self.session(key).json("POST", call, body=body)
                control = control or st == 200
                self.row(f"{rid}.propose.{plural}", key, f"POST {call} as mo-{key}",
                         "200: the dry run of an operator's proposal", f"{st} {str(raw)[:160]}" if st != 200 else "200",
                         st == 200)
            for org in data:
                if plural == "rolebindings":
                    away = next(o for o in [*data, self.installation] if o and o != org)
                    body = body | {"spec": body["spec"] | {"scope": {"organization": away}}}
                st, raw = self.session(f"{org}-admin").json("POST", call, body=body)
                self.row(f"operators.{org}-admin.propose.{plural}", org, f"POST {call} as mo-{org}-admin",
                         "403/404 (the operator's dry run 200)", f"{st} {str(raw)[:120]}" if st not in DENIED else str(st),
                         (st in DENIED) if control else None, "" if control else "no control: the operator's dry run failed")
        # The pages that answer with the installation's Organization: an operator's answer.
        everyone = {f"mo-{o}-{k}" for o in self.exp for k in ("admin", "member")}
        for key, rid in operators.items():
            st, body = self.session(key).json("GET", "/api/v1/organization/people?max=100")
            names = set(users(i.get("email", "") for i in (body or {}).get("items", []))) if st == 200 else set()
            self.row(f"{rid}.people", key, f"GET /api/v1/organization/people as mo-{key}",
                     "200 naming the people of every Organization", f"{st}, missing {len(everyone - names)}",
                     st == 200 and everyone <= names)
            for path in ("/api/v1/organization/widgets", "/api/v1/organization/setup"):
                st, body = self.session(key).json("GET", path)
                self.row(f"{rid}.{path}", key, f"GET {path} as mo-{key}", "200",
                         f"{st} {str(body)[:120]}" if st != 200 else "200", st == 200)
            st, _, raw = self.session(key).call("GET", "/api/v1/", headers={"OData-MaxVersion": "4.0",
                                                                             "Accept": "application/json"})
            self.row(f"{rid}.odata", key, f"GET /api/v1/ (OData service document) as mo-{key}",
                     "200, or 404 naming no Organization (path mode has no Organization of the request)",
                     f"{st} {raw[:120]!r}", st == 200 or (st == 404 and not any(o in raw.decode(errors='replace') for o in data)))
        for org in data:
            for plural in ("rolebindings", "groups", "organizations"):
                call = f"/api/v1/projects/org/{plural}"
                st, body = self.session(f"{org}-admin").json("GET", call)
                text = json.dumps(body, ensure_ascii=False)
                foreign = sorted(o for o in self.exp if o != org and (f"multiorg-{o}-" in text or f'"name": "{o}"' in text))
                self.row(f"operators.{org}-admin.org-namespace.{plural}", org, f"GET {call} as mo-{org}-admin",
                         "403/404, or 200 naming nothing of another Organization", f"{st}, foreign {foreign[:4]}",
                         st in DENIED or (st == 200 and not foreign))

    def template(self, rid: str) -> str:
        """A row id with Organization and project names as {o} and object names as {n}: the rows
        one check makes for each Organization or object share it."""
        if not hasattr(self, "_names"):
            names = {}
            for e in self.exp.values():
                for o in e["projects"].values():
                    for n in [*o["Endpoint"], *o["Pipeline"], *o["Dashboard"], *o["ckanOrgs"], *o["closed"], *o.get("spaces", [])]:
                        names[n] = "{n}"
            for org, e in self.exp.items():
                names.update({p: "{o}" for p in e["projects"]} | {org: "{o}"})
            alt = "|".join(re.escape(n) for n in sorted(names, key=len, reverse=True))
            self._names = (names, re.compile(rf"(?<![A-Za-z0-9_])(?:{alt})(?![A-Za-z0-9_])") if names else None)
        names, rx = self._names
        return rx.sub(lambda m: names[m.group(0)], rid) if rx else rid

    def count(self, full: bool):
        """The run's counters: rows per ordered pair (`setup.pairs` fails on a pair without one, on a
        full run) and skips per row template against skip-bound.json (a template over its bound
        fails; a template the bound does not name may skip nothing)."""
        orgs = list(self.exp)
        pairs = {(a, b): 0 for a in orgs for b in orgs if a != b}
        for r in self.rows:
            a, sep, b = str(r["target"]).partition("->")
            if sep and (a, b) in pairs and r["result"] != "skip":
                pairs[(a, b)] += 1
        missing = sorted(f"{a}->{b}" for (a, b), n in pairs.items() if not n)
        if full and pairs:
            self.row("setup.pairs", f"{len(pairs)} pairs", "rows per ordered pair of Organizations",
                     ">= 1 isolation row for each", f"{len(pairs) - len(missing)} of {len(pairs)}; none for {missing[:6]}",
                     not missing)
        skips: dict[str, int] = {}
        for r in self.rows:
            if r["result"] == "skip":
                t = self.template(r["id"])
                skips[t] = skips.get(t, 0) + 1
        mode = "hosts" if self.hosts else "path"
        bound = dict(json.loads(BOUND.read_text())[mode]) if BOUND.is_file() else {}
        if self.live:  # a live run adds no Dashboard (no write outside the fixtures): an Organization
            # whose export holds none of its own has nothing to judge there
            bound["dashboards.{o}"] = bound.get("dashboards.{o}", 0) + sum(
                1 for e in self.exp.values() if not any(p["Dashboard"] for p in e["projects"].values()))
        for t, n in sorted(skips.items()):
            if n > bound.get(t, 0):
                self.row(f"skips.{t}", mode, "skips of one row template", f"<= {bound.get(t, 0)} (skip-bound.json)",
                         str(n), False)
        return {"pairs": f"{len(pairs) - len(missing)}/{len(pairs)}", "skipsByTemplate": skips}

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
                own = min(self.exp[org]["projects"], default=None) if org else "helsinki"
                foreign = min((min(o["projects"]) for k, o in self.exp.items() if k != org and o["projects"]), default=None) if org else None
                pages = ([("own-project", f"/projects/{own}")] if own else []) + [("projects", "/"), ("organization", "/organization")]
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
        file = self.export / "manifest.json"
        manifest = json.loads(file.read_text()) if file.is_file() else {}
        if "multiorg" not in manifest:
            # --composed (the smoke gate composed the bundle): a missing one is a failure, or the gate
            # would go green without judging a single Organization.
            self.row("setup", "all", f"read {file}", "a multiorg block (compose.py)",
                     "none" if file.is_file() else "no manifest.json", False if self.a.composed else None,
                     "" if self.a.composed else
                     "not applicable: the export is one installation, not a composed multi-organization bundle")
            return
        self.expectations()
        if not self.state and not self.live:
            self.row("setup", "all", "-", "--state", "none", None, "no --state: persons cannot be made")
            return
        try:
            self.checks()
        finally:
            if self.live:
                self.cleanup()

    def checks(self):
        try:
            self.setup()
        except Exception as ex:  # noqa: BLE001 - a setup failure is a row, the rest is skipped
            self.row("setup", "all", "persons and bindings", "made", str(ex)[:300], False)
            return
        if getattr(self.a, "make_personas", None):
            self.save_personas(Path(self.a.make_personas))
            return
        steps = (self.check_projects, self.check_hosts, self.check_project_reads, self.check_writes, self.check_people,
                 self.check_mcp, self.check_connectors, self.check_data, self.check_catalogue, self.check_portals,
                 self.check_dashboards, self.check_global, self.check_operators, self.check_forged)
        if self.a.shots:
            steps += (self.check_ui,)
        for step in steps:
            group = step.__name__.removeprefix("check_")
            if (self.a.only and group not in self.a.only.split(",")) or (self.live and group in LIVE_OFF):
                continue
            try:
                step()
            except Exception as ex:  # noqa: BLE001
                self.row(f"{step.__name__}.crashed", "all", step.__name__, "runs", repr(ex)[:300], False)

    def write(self, out: Path):
        out.mkdir(parents=True, exist_ok=True)
        made = getattr(self.a, "make_personas", None)  # setup alone: no pair to count
        counted = self.count(not self.a.only and not made) if self.exp else {}
        (out / "results.json").write_text(json.dumps({"suite": "multiorg", "base": self.base, "checks": self.rows},
                                                     indent=1))
        tools = {k: {"portal": v.get("tools"), "connector": v.get("connectorTools")}
                 for k, v in self.people.items() if v.get("tools") is not None or v.get("connectorTools")}
        (out / "mcp-tools.json").write_text(json.dumps(tools, indent=1))
        n = {r: sum(1 for x in self.rows if x["result"] == r) for r in ("pass", "fail", "skip")}
        (out / "counts.json").write_text(json.dumps(counted, indent=1))
        lines = ["# multiorg smoke", "", f"Base {self.base}; organizations {', '.join(self.exp)}; "
                 f"{n['pass']} pass, {n['fail']} fail, {n['skip']} skip; pairs {counted.get('pairs', '-')}; "
                 f"skips by template {json.dumps(counted.get('skipsByTemplate', {}))}.", "",
                 "| id | target | call | expected | actual | result |", "|---|---|---|---|---|---|"]
        order = {"fail": 0, "skip": 1, "pass": 2}
        for r in sorted(self.rows, key=lambda x: order[x["result"]]):
            cell = lambda s: str(s).replace("|", "\\|").replace("\n", " ")[:200]  # noqa: E731
            lines.append(f"| {cell(r['id'])} | {cell(r['target'])} | `{cell(r['call'])}` | {cell(r['expected'])} | "
                         f"{cell(r['actual'])} | {r['result']}{' (' + cell(r['reason']) + ')' if r['reason'] else ''} |")
        (out / "REPORT.md").write_text("\n".join(lines) + "\n")
        return n | counted


def main(argv):
    ap = argparse.ArgumentParser(prog="run multiorg")
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--export", required=True, help="bundle written by compose.py")
    ap.add_argument("--state", help="the rehearsal's state dir (realm admin client, forge token)")
    ap.add_argument("--admin-token-file", help="unused: persons are made through --state")
    ap.add_argument("--person-token-file", help="unused: persons are made through --state")
    ap.add_argument("--only", help="comma list of check groups: projects,hosts,project_reads,writes,people,mcp,connectors,data,catalogue,portals,dashboards,global,operators,forged,ui")
    ap.add_argument("--shots", action="store_true", help="browser shots 1440/400 per persona (Playwright)")
    ap.add_argument("--sync-timeout", type=int, default=180)
    ap.add_argument("--composed", action="store_true",
                    help="the bundle must be composed (the smoke gate): no multiorg block is a failure, not a skip")
    ap.add_argument("--jc-config", "--hosts", dest="jc_config",
                    help="the jc.toml (or its [[organizations]] fragment): host mode, each Organization on its own hosts")
    ap.add_argument("--live", action="store_true",
                    help="judge a live deployment: requests only from the allowlist, personas from --personas")
    ap.add_argument("--personas", help="--live: the personas file the owner-approved Operator Change made (0600)")
    ap.add_argument("--make-personas", help="on a local jc: make the personas and their bindings, write them here, stop")
    a = ap.parse_args(argv)
    if a.live:
        if not a.personas:
            ap.error("--live needs --personas")
        if a.make_personas or a.shots:
            ap.error("--live makes no personas and takes no shots")
        live_base(a.base_url)
    else:
        R.loopback_only(a.base_url)
    a.out.mkdir(parents=True, exist_ok=True)
    s = Suite(a)
    s.run()
    n = s.write(a.out)
    print(json.dumps(n))  # the run line: pass, fail, skip, pairs covered, skips per row template
    return 1 if n["fail"] else 0
