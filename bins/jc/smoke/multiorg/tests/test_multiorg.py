"""Unit tests of the multiorg suite's host mode (the live proof is a rehearsal run).

    python3 -m pytest bins/jc/smoke/multiorg/tests -q
"""

import argparse
import importlib.util
import json
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent

_spec = importlib.util.spec_from_file_location("smoke_multiorg_suite", HERE.parent / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

TOML = """
[[organizations]]
name = "hel"
catalogue_id = "00000000-0000-7000-8000-0000000000a1"
hosts = { portal = "Portal.Hel.localhost:42301", opendata = "opendata.hel.localhost:42301", api = "api.hel.localhost:42301", mcp = "mcp.hel.localhost:42301" }
"""


def test_the_hosts_of_each_organization_are_read_lowercased(tmp_path):
    (tmp_path / "jc.toml").write_text(TOML)
    hosts = suite.organization_hosts(tmp_path / "jc.toml")
    assert hosts == {"hel": {"portal": "portal.hel.localhost:42301", "opendata": "opendata.hel.localhost:42301",
                             "api": "api.hel.localhost:42301", "mcp": "mcp.hel.localhost:42301"}}
    (tmp_path / "none.toml").write_text("")
    assert suite.organization_hosts(tmp_path / "none.toml") == {}


def test_a_request_goes_to_loopback_with_its_hosts_header_and_jar(monkeypatch):
    sent = []

    def call(self, method, path, body=None, headers=None, form=None, timeout=60):
        sent.append((method, path, headers["Host"], id(self.jar)))
        self.jar["seen"] = headers["Host"]

    monkeypatch.setattr(suite.R.Http, "call", call)
    http = suite.HostHttp("http://127.0.0.1:42301", "portal.hel.localhost:42301",
                          ["portal.hel.localhost:42301", "portal.ba.localhost:42301"])
    http.call("GET", "/api/v1/projects?x=1")
    http.call("GET", "http://portal.ba.localhost:42301/api/v1/me")
    http.call("GET", "/api/v1/me")
    assert [s[:3] for s in sent] == [
        ("GET", "http://127.0.0.1:42301/api/v1/projects?x=1", "portal.hel.localhost:42301"),
        ("GET", "http://127.0.0.1:42301/api/v1/me", "portal.ba.localhost:42301"),
        ("GET", "http://127.0.0.1:42301/api/v1/me", "portal.hel.localhost:42301"),
    ]
    # One jar per host: hel's cookies are never sent to ba.
    assert sent[0][3] == sent[2][3] != sent[1][3]
    assert http.jars["portal.ba.localhost:42301"] == {"seen": "portal.ba.localhost:42301"}


def test_an_address_off_this_jc_is_refused_and_never_sent(monkeypatch):
    monkeypatch.setattr(suite.R.Http, "call", lambda *a, **k: pytest.fail("sent"))
    http = suite.HostHttp("http://127.0.0.1:42301", "portal.hel.localhost:42301", ["portal.hel.localhost:42301"])
    with pytest.raises(RuntimeError, match="not a host of this jc"):
        http.call("GET", "https://dev.joinedcontext.com/api/v1/me")


def args(tmp_path, state=None):
    exp = tmp_path / "export"
    exp.mkdir()
    (exp / "manifest.json").write_text(json.dumps({"installation": "prod"}))
    return argparse.Namespace(base_url="http://127.0.0.1:9", state=state, export=str(exp), jc_config=None,
                              composed=False)


def test_an_export_without_a_multiorg_block_is_one_named_skip(tmp_path):
    s = suite.Suite(args(tmp_path))
    s.run()
    assert [(r["id"], r["result"]) for r in s.rows] == [("setup", "skip")]
    assert s.rows[0]["reason"].startswith("not applicable: the export is one installation")


def test_a_gate_bundle_that_is_not_composed_or_missing_fails_instead_of_skipping(tmp_path):
    a = args(tmp_path)
    a.composed = True
    s = suite.Suite(a)
    s.run()
    assert [(r["id"], r["result"], r["actual"]) for r in s.rows] == [("setup", "fail", "none")]
    a.export = str(tmp_path / "gone")
    s = suite.Suite(a)
    s.run()
    assert [(r["id"], r["result"], r["actual"]) for r in s.rows] == [("setup", "fail", "no manifest.json")]


def test_the_suite_takes_the_state_realm(tmp_path):
    state = tmp_path / "state"
    state.mkdir()
    (state / "realm-name.txt").write_text("smartcity")
    assert suite.Suite(args(tmp_path, str(state))).realm == "smartcity"


def test_an_organization_endpoint_counts_as_readable_only_with_a_read_policy_in_its_space(tmp_path):
    a = args(tmp_path)
    exp = Path(a.export)
    (exp / "manifest.json").write_text(json.dumps({"multiorg": {"organizations": {"zilina": {"projects": ["zilina"]}}}}))
    (exp / "gitea/files/joinedcontext/configuration/organizations").mkdir(parents=True)
    for space, slug, verb in (("mesto", "aaaa", "createEntity"), ("data", "bbbb", "retrieveOps")):
        d = exp / "gitea/files/joinedcontext/zilina/spaces" / space
        (d / "endpoints").mkdir(parents=True)
        (d / "policies").mkdir()
        (d / "endpoints/e.yaml").write_text(f"kind: Endpoint\nmetadata:\n  name: {space}\nspec:\n  slug: {slug}\n"
                                            "  audience: organization\n")
        (d / "policies/p.yaml").write_text(f"kind: Policy\nspec:\n  operations:\n    - {verb}\n")
    s = suite.Suite(a)
    s.expectations()
    project = s.exp["zilina"]["projects"]["zilina"]
    assert set(project["closed"]) == {"aaaa", "bbbb"} and project["readable"] == ["bbbb"]


def test_the_people_rows_name_user_names_never_e_mail_addresses(tmp_path):
    # Smoke 43161732041: the logs suite failed multiorg's REPORT.md and results.json on the
    # addresses these rows printed (personal.results8, personal.results9).
    everyone = [{"email": f"mo-{o}-{k}@rehearsal.invalid"} for o in ("hel", "zilina") for k in ("admin", "member")]

    class Session:
        def json(self, method, path):
            return 200, {"items": everyone}

    s = suite.Suite(args(tmp_path))
    s.exp, s.installation = {"hel": {}, "zilina": {}}, "hel"
    s.session = lambda key, on=None: Session()
    s.check_people()
    assert s.rows and not any("@" in str(r[k]) for r in s.rows for k in ("expected", "actual", "reason"))
    hel = next(r for r in s.rows if r["id"] == "people.hel-admin.no-foreign")
    assert hel["actual"] == "200, foreign ['mo-zilina-admin', 'mo-zilina-member']"


def counted(tmp_path, rows, exp, full=True):
    d = tmp_path / str(len(list(tmp_path.iterdir())))
    d.mkdir()
    s = suite.Suite(args(d))
    s.exp, s.rows = exp, rows
    return s, s.count(full)


def row(id_, target, result="pass"):
    return {"id": id_, "target": target, "call": "-", "expected": "-", "actual": "-", "result": result, "reason": ""}


EMPTY = {"Endpoint": [], "Pipeline": [], "CkanInstance": [], "Dashboard": [], "ckanOrgs": [], "closed": {}, "readable": []}


def test_a_bundle_with_a_pair_of_organizations_without_a_row_fails_setup(tmp_path):
    exp = {"hel": {"projects": {"helsinki": EMPTY}}, "zilina": {"projects": {"zilina": EMPTY}}, "ctl-a": {"projects": {}}}
    rows = [row("isolation.hel-admin.zilina/", "hel->zilina"), row("isolation.zilina-admin.helsinki/", "zilina->hel"),
            row("global.hel-admin.none-of.ctl-a", "hel->ctl-a"), row("global.ctl-a-admin.none-of.hel", "ctl-a->hel"),
            row("global.ctl-a-admin.none-of.zilina", "ctl-a->zilina"),
            # a skipped row covers no pair
            row("global.zilina-admin.none-of.ctl-a", "zilina->ctl-a", "skip")]
    s, n = counted(tmp_path, [dict(r) for r in rows], exp)
    pairs = next(r for r in s.rows if r["id"] == "setup.pairs")
    assert (pairs["result"], n["pairs"]) == ("fail", "5/6")
    assert "zilina->ctl-a" in pairs["actual"]
    rows[-1]["result"] = "pass"
    s, n = counted(tmp_path, [dict(r) for r in rows], exp)
    assert next(r for r in s.rows if r["id"] == "setup.pairs")["result"] == "pass" and n["pairs"] == "6/6"


def test_skips_are_counted_per_row_template_and_may_not_outgrow_the_bound(tmp_path):
    bound = json.loads(suite.BOUND.read_text())
    assert bound["path"]["people.{o}-admin.own"] == 2  # the three-Organization bundle's two
    exp = {o: {"projects": {p: EMPTY}} for o, p in (("hel", "helsinki"), ("zilina", "zilina"), ("bbsk", "bbsk"))}
    rows = [row(f"people.{o}-admin.own", o, "skip") for o in ("zilina", "bbsk")]
    s, n = counted(tmp_path, [dict(r) for r in rows], exp, full=False)
    assert n["skipsByTemplate"] == {"people.{o}-admin.own": 2} and not [r for r in s.rows if r["result"] == "fail"]
    rows.append(row("people.hel-admin.own", "hel", "skip"))
    # a template the bound does not name may skip nothing
    rows.append(row("operators.hel-admin.propose.groups", "hel", "skip"))
    s, n = counted(tmp_path, rows, exp, full=False)
    over = sorted((r["id"], r["actual"]) for r in s.rows if r["result"] == "fail" and r["id"].startswith("skips."))
    assert over == [("skips.operators.{o}-admin.propose.groups", "1"), ("skips.people.{o}-admin.own", "3")]


def test_a_template_names_organizations_and_projects_alike_and_the_longest_name_first(tmp_path):
    s = suite.Suite(args(tmp_path))
    s.exp = {"hel": {"projects": {"helsinki": EMPTY, "helsinki-mobility": EMPTY | {"Endpoint": ["hel-bikes"]}}},
             "zilina": {"projects": {"zilina": EMPTY}}}
    assert s.template("isolation.hel-admin.zilina/rolebindings") == s.template("isolation.zilina-admin.helsinki/rolebindings") \
        == "isolation.{o}-admin.{o}/rolebindings"
    assert s.template("data.zilina-member.read.helsinki-mobility/hel-bikes") == "data.{o}-member.read.{o}/{n}"


def test_global_reads_give_each_pair_a_row_also_for_an_empty_organization(tmp_path):
    seen = {"ctl-a-admin": "mo-hel-admin"}  # ctl-a's admin is shown hel's admin in one answer

    class Session:
        def __init__(self, key):
            self.key = key

        def json(self, method, path):
            return 200, {"items": [seen.get(self.key, "")]} if path.endswith("/people") else {}

    s = suite.Suite(args(tmp_path))
    s.exp = {"hel": {"projects": {"helsinki": EMPTY}}, "ctl-a": {"projects": {}}}
    s.session = lambda key, on=None: Session(key)
    s.GLOBAL_READS = ["/api/v1/notifications", "/api/v1/organization/people"]
    s.check_global = suite.Suite.check_global.__get__(s)
    s.foreign_marks = lambda org: {f"mo-{o}-{k}": o for o in s.exp if o != org for k in ("admin", "member")}
    s.check_global()
    pair = {r["id"]: r["result"] for r in s.rows if ".none-of." in r["id"]}
    assert pair == {"global.hel-admin.none-of.ctl-a": "pass", "global.hel-member.none-of.ctl-a": "pass",
                    "global.ctl-a-admin.none-of.hel": "fail", "global.ctl-a-member.none-of.hel": "pass"}


_cspec = importlib.util.spec_from_file_location("smoke_multiorg_compose", HERE.parent / "compose.py")
compose = importlib.util.module_from_spec(_cspec)
_cspec.loader.exec_module(compose)


def export(root, orgs, projects, installation=None):
    """A minimal export: Organization manifests, projects (name -> org) with one space of 3 Endpoints."""
    cfg = root / "gitea/files/joinedcontext/configuration"
    (cfg / "organizations").mkdir(parents=True)
    (cfg / "projects").mkdir()
    for o in orgs:
        flag = "  installation: true\n" if o == installation else ""
        (cfg / "organizations" / f"{o}.yaml").write_text(f"kind: Organization\nmetadata:\n  name: {o}\nspec:\n{flag}  domain: {o}.fi\n")
    repos = [{"owner": "joinedcontext", "name": "configuration"}]
    for p, o in projects.items():
        (cfg / "projects" / f"{p}.yaml").write_text(f"kind: Project\nmetadata:\n  name: {p}\nspec:\n  organizationRef: {o}\n")
        d = root / "gitea/files/joinedcontext" / p
        (d / "spaces/s/endpoints").mkdir(parents=True)
        (d / "project.yaml").write_text(f"kind: Project\nmetadata:\n  name: {p}\n  title: {{ en: City {p} }}\n"
                                        f"spec:\n  organizationRef: {o}\n  domain: {p}.sk\n")
        for e in ("a", "b", "c"):
            (d / f"spaces/s/endpoints/{e}.yaml").write_text("kind: Endpoint\n")
        repos.append({"owner": "joinedcontext", "name": p})
    (root / "gitea/repos.json").write_text(json.dumps(repos))
    (root / "keycloak").mkdir()
    (root / "grafana/dashboards").mkdir(parents=True)
    (root / "antares").mkdir()
    (root / "antares/entities.jsonl").write_text("")
    (root / "antares/temporal.jsonl").write_text("")
    (root / "manifest.json").write_text(json.dumps({"exportedAt": "2026-11-14T00:00:00Z"}))
    return root


def test_compose_makes_organizations_of_named_projects_and_empty_controls_with_no_installation(tmp_path):
    src = export(tmp_path / "src", ["hel"], {"helsinki": "hel", "bbsk": "hel", "praha": "hel"}, installation="hel")
    orgs = compose.parse_orgs(["hel=" + str(src) + ":helsinki", f"bbsk={src}:bbsk", "ctl-a="])
    summary = compose.compose(src, orgs, tmp_path / "out", 0, per_space=2, installation=False)
    assert {o: v["projects"] for o, v in summary["organizations"].items()} == {"hel": ["helsinki"], "bbsk": ["bbsk"], "ctl-a": []}
    cfg = tmp_path / "out/gitea/files/joinedcontext/configuration"
    assert sorted(f.stem for f in (cfg / "organizations").glob("*.yaml")) == ["bbsk", "ctl-a", "hel"]
    assert not any("installation" in f.read_text() for f in (cfg / "organizations").glob("*.yaml"))
    bbsk = (cfg / "organizations/bbsk.yaml").read_text()
    assert "name: bbsk" in bbsk and "domain: bbsk.sk" in bbsk and "title: { en: City bbsk }" in bbsk
    assert "domain: ctl-a.example" in (cfg / "organizations/ctl-a.yaml").read_text()
    assert "organizationRef: bbsk" in (cfg / "projects/bbsk.yaml").read_text()
    assert "organizationRef: bbsk" in (tmp_path / "out/gitea/files/joinedcontext/bbsk/project.yaml").read_text()
    assert not (cfg / "projects/praha.yaml").exists()
    assert sorted(f.name for f in (tmp_path / "out/gitea/files/joinedcontext/bbsk/spaces/s/endpoints").iterdir()) == ["a.yaml", "b.yaml"]
    assert summary["endpointsDropped"] == 2
    # the installation's domain stays what {orgDomain} renders with: an overlay, no id changes
    env = (cfg / "environments/rehearsal.yaml").read_text()
    assert "kind: Environment" in env and "orgDomain: hel.fi" in env and summary["environment"]["orgDomain"] == "hel.fi"


def test_compose_refuses_a_project_named_twice_or_missing_and_a_bad_name(tmp_path):
    src = export(tmp_path / "src", ["hel"], {"helsinki": "hel"})
    with pytest.raises(SystemExit, match="named for both"):
        compose.compose(src, compose.parse_orgs([f"hel={src}", f"bbsk={src}:helsinki"]), tmp_path / "o", 0)
    with pytest.raises(SystemExit, match="holds no project praha"):
        compose.compose(src, compose.parse_orgs([f"praha={src}:praha"]), tmp_path / "o", 0)
    with pytest.raises(SystemExit, match="lower-case Organization name"):
        compose.parse_orgs(["Hel=x"])


def live_allowlist(tmp_path):
    g = suite.Allowlist(tmp_path / "requests.log", {"jc.jcdev.dev.joinedcontext.com"}, fixtures={"ctl-a-fixture"},
                        personas={"u-1"})
    g.learn({"result": {"tools": [{"name": "jc_resource_list", "annotations": {"readOnlyHint": True}},
                                  {"name": "jc_resource_delete", "annotations": {"readOnlyHint": False}},
                                  {"name": "jc_unannotated"}]}})
    return g


B = "https://jc.jcdev.dev.joinedcontext.com"


def test_live_writes_outside_the_fixtures_are_refused_before_anything_is_sent(tmp_path, monkeypatch):
    g = live_allowlist(tmp_path)
    sent = []
    monkeypatch.setattr(suite.R.Http, "call", lambda self, m, p, *a, **k: sent.append((m, p)) or (200, {}, b"{}"))
    h = g.guard(suite.R.Http(B))
    refused = [("DELETE", "/api/v1/projects/helsinki/endpoints/air"), ("PUT", "/api/v1/projects/helsinki/endpoints/air"),
               ("PATCH", "/api/v1/projects/zilina/spaces/s"), ("POST", "/api/v1/connectors"),
               ("POST", "/id/admin/realms/dev/users"), ("DELETE", "/id/admin/realms/dev/users/someone-else"),
               ("POST", "/api/v1/projects/org/organizations?dryRun=All")]
    for method, path in refused:
        with pytest.raises(suite.Refused):
            h.call(method, path, body={})
    for name in ("jc_resource_delete", "jc_unannotated"):  # a write tool, and one that does not say
        with pytest.raises(suite.Refused):
            h.call("POST", "/api/v1/mcp", body={"jsonrpc": "2.0", "method": "tools/call", "params": {"name": name}})
    with pytest.raises(suite.Refused):  # another host, even with a GET
        h.call("GET", "https://evil.example/api/v1/projects")
    assert sent == []
    allowed = [("GET", "/api/v1/projects/helsinki/endpoints", None),
               ("POST", "/id/realms/dev/protocol/openid-connect/token", None),
               ("POST", "/id/realms/dev/login-actions/authenticate?session_code=x", None),
               ("POST", "/api/endpoint/abcd/ngsi-ld/v1/entityOperations/query", {}),
               ("POST", "/api/v1/mcp", {"method": "tools/list"}),
               ("POST", "/api/v1/mcp", {"method": "tools/call", "params": {"name": "jc_resource_list"}}),
               ("DELETE", "/api/v1/projects/ctl-a-fixture/endpoints/e", None),
               ("DELETE", "/id/admin/realms/dev/users/u-1", None)]
    for method, path, body in allowed:
        h.call(method, path, body=body)
    assert len(sent) == len(allowed)
    log = (tmp_path / "requests.log").read_text().splitlines()
    assert sum(line.endswith("REFUSED") for line in log) == len(refused) + 3
    assert not any("session_code" in line for line in log)  # paths without their query


def test_live_mode_takes_only_the_live_hosts_or_loopback():
    suite.live_base("https://smartcity.joinedcontext.com")
    suite.live_base("http://127.0.0.1:42240")
    for base in ("https://dev.joinedcontext.com", "http://smartcity.joinedcontext.com",
                 "https://smartcity.joinedcontext.com.evil.example", "https://smartcity.joinedcontext.com:8443"):
        with pytest.raises(SystemExit):
            suite.live_base(base)
    with pytest.raises(SystemExit, match="refusing"):  # without --live: loopback only, as before
        suite.main(["--base-url", "https://smartcity.joinedcontext.com", "--out", "/nonexistent/x", "--export", "x"])


def test_a_live_run_deletes_its_personas_and_their_credentials_also_after_a_crash(tmp_path, monkeypatch):
    a = args(tmp_path, str(tmp_path))
    exp = Path(a.export)
    (exp / "manifest.json").write_text(json.dumps({"multiorg": {"organizations": {"ctl-a": {"projects": []}}}}))
    (exp / "gitea/files/joinedcontext/configuration/organizations").mkdir(parents=True)
    d = tmp_path / "personas"
    d.mkdir(mode=0o700)
    creds = d / "p.json"
    creds.write_text(json.dumps({k: {"username": f"mo-{k}", "id": f"id-{k}", "password": "x"}
                                 for k in ("platform", "ctl-a-admin", "ctl-a-member")}))
    creds.chmod(0o600)
    a.live, a.personas, a.out, a.only, a.make_personas = True, str(creds), tmp_path / "out", None, None
    a.out.mkdir()
    deleted = []

    def call(self, method, path, *rest, **kw):
        if method == "DELETE":
            deleted.append(path.rsplit("/", 1)[1])
            return 204, {}, b""
        return 200, {}, b"{}"

    monkeypatch.setattr(suite.R.Http, "call", call)
    monkeypatch.setattr(suite.R, "admin_token", lambda h, state: "t")
    s = suite.Suite(a)
    s.synced = lambda: (_ for _ in ()).throw(RuntimeError("the deployment went away"))
    s.run()
    assert sorted(deleted) == ["id-ctl-a-admin", "id-ctl-a-member", "id-platform"]
    assert not creds.exists()
    assert json.loads((a.out / "cleanup.json").read_text()) == {"personas": [], "deleted": 3}
    assert [r["result"] for r in s.rows if r["id"] in ("setup", "live.cleanup")] == ["fail", "pass"]
    assert all(line.split()[-1] in ("read", "sign-in", "cleanup") for line in (a.out / "requests.log").read_text().splitlines())


def test_a_personas_file_others_can_read_is_refused(tmp_path):
    s = suite.Suite(args(tmp_path))
    s.exp = {"ctl-a": {"projects": {}}}
    f = tmp_path / "p.json"
    f.write_text("{}")
    f.chmod(0o644)
    with pytest.raises(RuntimeError, match="readable by others"):
        s.personas_from(f)


def test_a_live_run_may_skip_the_dashboards_of_organizations_whose_export_holds_none(tmp_path):
    exp = {"hel": {"projects": {"helsinki": EMPTY | {"Dashboard": ["d"]}}}, "zilina": {"projects": {"zilina": EMPTY}}}
    rows = [row("dashboards.zilina", "zilina", "skip")]
    s, _ = counted(tmp_path, [dict(r) for r in rows], exp, full=False)
    assert [r["id"] for r in s.rows if r["result"] == "fail"] == ["skips.dashboards.{o}"]  # the gate adds one
    d = tmp_path / "live"
    d.mkdir()
    s = suite.Suite(args(d))
    s.exp, s.rows, s.live = exp, [dict(r) for r in rows], True
    s.count(False)
    assert not [r for r in s.rows if r["result"] == "fail"]
