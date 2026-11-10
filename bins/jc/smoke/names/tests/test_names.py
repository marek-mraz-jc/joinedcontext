"""Selftests of the names suite on fixture listings (the live proof is a rehearsal run).

    python3 -m pytest bins/jc/smoke/names/tests -q
"""

import importlib.util
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
NAMES = HERE.parent
sys.path.insert(0, str(NAMES))
sys.path.insert(0, str(NAMES.parent))

import compare as cmp
import sources
from common import FAIL, PASS, SKIP, Results

_spec = importlib.util.spec_from_file_location("smoke_names_suite", NAMES / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)


def rows_by_target(res):
    return {r["target"]: r for r in res.rows}


def renames_file(tmp_path, *rows):
    f = tmp_path / "RENAMES.md"
    f.write_text("# Renames\n\n| kind | old | new | why | how old clients keep working |\n|---|---|---|---|---|\n"
                 + "".join(f"| {' | '.join(r)} |\n" for r in rows))
    return cmp.load_renames(str(f))


def test_equal_listings_pass_and_report_nothing_extra():
    res = Results("names", "http://localhost")
    extra = cmp.compare(res, "project", ["a", "b"], {"a", "b"}, [], lambda o: (False, ""), "GET x")
    assert extra == [] and [r["result"] for r in res.rows] == [PASS, PASS]


def test_a_missing_prod_name_fails_and_names_what_jc_has_instead():
    res = Results("names", "http://localhost")
    extra = cmp.compare(res, "space", ["p/air", "p/bikes"], {"p/air", "p/bicycles", "q/x"}, [],
                        lambda o: (False, ""), "GET x")
    r = rows_by_target(res)
    assert r["p/air"]["result"] == PASS
    assert r["p/bikes"]["result"] == FAIL and "p/bicycles" in r["p/bikes"]["actual"]
    assert "q/x" not in r["p/bikes"]["actual"]
    # The extras are report lines, never rows that pass.
    assert extra == ["p/bicycles", "q/x"] and len(res.rows) == 2


def test_a_kept_components_tool_missing_on_jc_is_a_named_skip_and_only_that_tool():
    res = Results("names", "http://localhost")
    kept = {"/api/mcp#search_documents": "assistant"}
    cmp.compare(res, "tool", ["/api/mcp#search_documents", "/api/mcp#query_entities"], {"/api/mcp#list_types"},
                [], lambda o: (False, ""), "POST /api/mcp tools/list", kept=kept)
    r = rows_by_target(res)
    assert r["/api/mcp#search_documents"]["result"] == SKIP
    assert r["/api/mcp#search_documents"]["reason"] == "kept component: assistant"
    # A tool jc must serve itself still fails; a kept tool jc does list passes.
    assert r["/api/mcp#query_entities"]["result"] == FAIL
    res = Results("names", "http://localhost")
    cmp.compare(res, "tool", ["/api/mcp#search_documents"], {"/api/mcp#search_documents"}, [],
                lambda o: (False, ""), "x", kept=kept)
    assert res.rows[0]["result"] == PASS
    assert set(suite.KEPT_TOOLS) == {"search_documents"}


def test_an_extra_jc_name_alone_is_a_report_line_not_a_failure():
    res = Results("names", "http://localhost")
    extra = cmp.compare(res, "dataset", ["a"], {"a", "new-one"}, [], lambda o: (False, ""), "GET x")
    assert extra == ["new-one"] and res.totals()[FAIL] == 0


def test_a_listed_rename_passes_only_when_the_old_address_answers(tmp_path):
    renames = renames_file(tmp_path, ("endpoint", "old-slug", "new-slug", "slug too long", "alias route"),
                           ("endpoint", "gone-slug", "fresh-slug", "clash", "redirect"))
    asked = []

    def answers(old):
        asked.append(old)
        return (old == "old-slug"), f"GET /api/endpoint/{old}/ngsi-ld/v1/types {200 if old == 'old-slug' else 404}"

    res = Results("names", "http://localhost")
    extra = cmp.compare(res, "endpoint", ["old-slug", "gone-slug", "same"], {"new-slug", "fresh-slug", "same"},
                        renames, answers, "GET x")
    r = rows_by_target(res)
    assert r["old-slug"]["result"] == PASS and "RENAMES.md" in r["old-slug"]["reason"]
    assert r["gone-slug"]["result"] == FAIL and "404" in r["gone-slug"]["actual"]
    assert r["same"]["result"] == PASS
    assert sorted(asked) == ["gone-slug", "old-slug"]
    # The new names of listed renames are no extras.
    assert extra == []


def test_a_listed_rename_whose_new_name_jc_lacks_fails(tmp_path):
    renames = renames_file(tmp_path, ("app", "p/a", "p/b", "why", "redirect"))
    res = Results("names", "http://localhost")
    cmp.compare(res, "app", ["p/a"], set(), renames, lambda o: (True, "200"), "GET x")
    assert res.rows[0]["result"] == FAIL and "NOT listed" in res.rows[0]["actual"]


def test_an_unreadable_jc_listing_fails_every_prod_name():
    res = Results("names", "http://localhost")
    cmp.compare(res, "client", ["r/a", "r/b"], None, [], lambda o: (True, ""), "GET x", "403 refused")
    assert [r["result"] for r in res.rows] == [FAIL, FAIL] and "403" in res.rows[0]["actual"]


def test_no_renames_file_means_no_renames_and_a_bad_row_is_refused(tmp_path):
    assert cmp.load_renames(None) == []
    f = tmp_path / "R.md"
    f.write_text("| kind | old | new | why | how |\n|---|---|---|---|---|\n| endpoint | a | b | | |\n")
    with pytest.raises(SystemExit):
        cmp.load_renames(str(f))
    f.write_text("| kind | old | new | why | how |\n|---|---|---|---|---|\n| slugz | a | b | c | d |\n")
    with pytest.raises(SystemExit):
        cmp.load_renames(str(f))


def test_an_extra_row_is_read_as_its_kind_and_never_stands_in_for_a_missing_name(tmp_path):
    renames = renames_file(tmp_path, ("dataset", "catalog", "zilina-catalog", "why", "alias"),
                           ("dataset (extra)", "(none)", "praha-catalog", "why", "nothing to keep"))
    assert [(r.kind, r.new, r.extra) for r in renames] == [("dataset", "zilina-catalog", False),
                                                           ("dataset", "praha-catalog", True)]
    res = Results("names", "http://localhost")
    cmp.compare(res, "dataset", ["catalog", "(none)"], {"zilina-catalog", "praha-catalog"}, renames,
                lambda o: (True, "200"), "GET x")
    by = rows_by_target(res)
    assert by["catalog"]["result"] == PASS and by["(none)"]["result"] == FAIL
    stale = Results("names", "http://localhost")
    cmp.stale_renames(stale, renames, {"dataset": {"catalog"}})
    assert stale.rows == []
    cmp.stale_renames(stale, renames, {"dataset": {"catalog", "praha-catalog"}})
    assert [r["target"] for r in stale.rows] == ["dataset (extra) praha-catalog"]
    f = tmp_path / "R.md"
    f.write_text("| kind | old | new | why | how |\n|---|---|---|---|---|\n| dataset (extra) | catalog | x | c | d |\n")
    with pytest.raises(SystemExit):
        cmp.load_renames(str(f))


def test_a_rename_of_a_name_prod_lacks_is_a_failing_row(tmp_path):
    renames = renames_file(tmp_path, ("project", "nope", "x", "why", "how"))
    res = Results("names", "http://localhost")
    cmp.stale_renames(res, renames, {"project": {"a"}})
    assert res.rows[0]["result"] == FAIL


def test_tool_tables_are_read_from_both_constructor_forms():
    text = '''
pub(crate) const ANONYMOUS_TOOLS: &[&str] = &[
    "list_endpoints",
    "get_entity",
];
pub(crate) const TOOLS: [ToolDef; 2] = [
    write(
        "append_attributes",
        OperationId::AppendAttributes,
        "Append (TS 104 175).",
    ),
    ToolDef {
        name: "area_overview",
        op: OperationId::QueryEntities,
    },
];
const OTHER: &[&str] = &["not_a_tool"];
'''
    assert sources.tool_table(text) == ["append_attributes", "area_overview"]
    assert sources.string_list(text, "ANONYMOUS_TOOLS") == ["list_endpoints", "get_entity"]


def test_this_clones_tool_sources_hold_the_gateway_and_broker_tables():
    mine = sources.jc_source_tools(suite.REPO)
    assert "list_endpoints" in mine["anonymous"] and "create_entity" in mine["gateway"]
    assert "query_entities" in mine["broker"]


# ---------------------------------------------------------------- the whole run on a fixture export

def write_export(root: Path):
    files = root / "gitea" / "files" / "joinedcontext"
    (root / "antares").mkdir(parents=True)
    (root / "keycloak").mkdir()
    (root / "manifest.json").write_text(json.dumps({"exportedAt": "2026-11-08T00:00:00Z"}))
    (root / "keycloak" / "realm-city.json").write_text(json.dumps(
        {"realm": "city", "clients": [{"clientId": "portal-api"}, {"clientId": "gone"}]}))
    p = files / "p"
    (p / "spaces" / "air" / "endpoints").mkdir(parents=True)
    (p / "mcp").mkdir()
    (p / "project.yaml").write_text("kind: Project\nmetadata: {name: p}\n")
    (p / "spaces" / "air" / "space.yaml").write_text("kind: ContextSpace\nmetadata: {name: air, namespace: p}\nspec: {}\n")
    (p / "spaces" / "air" / "endpoints" / "open.yaml").write_text(
        "kind: Endpoint\nmetadata: {name: open, namespace: p}\nspec:\n  contextSpaceRef: air\n"
        "  slug: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n  audience: public\n  enabledRepresentations: [ngsi-ld, mcp]\n"
        "  publish: {ckan: {name: air-data}}\n")
    (p / "mcp" / "open.yaml").write_text("kind: McpServer\nmetadata: {name: open, namespace: p}\nspec: {audience: public}\n")


class FakeJc:
    """jc's listings as fixtures: what a run reads, without a server."""

    def __init__(self, missing=()):
        self.missing = set(missing)
        self.http = self

    def call(self, method, path, **_):
        return (200 if "package_show" in path else 404), {}, b""

    def projects(self):
        return {"p", "extra-project"}, ""

    def resources(self, projects, plural):
        rows = {"spaces": {"p/air": {}}, "apps": {}, "dashboards": {},
                "endpoints": {"p/open": {"metadata": {"name": "open"}, "spec": {
                    "slug": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "enabledRepresentations": ["ngsi-ld", "mcp"]}}}}[plural]
        return {k: v for k, v in rows.items() if k not in self.missing}, ""

    def portal_answers(self, path):
        return False, "404"

    def statuses(self, calls):
        out = {}
        for m, path in calls:
            known = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" in path or path == "/api/mcp" or path == "/api/mcp/p/open"
            # As jc: an Endpoint's MCP door asks for a token before it looks the slug up.
            if m == "POST" and path.startswith("/api/endpoint/") and not known:
                out[(m, path)] = 401
                continue
            out[(m, path)] = 404 if not known or path in self.missing else 200
        return out

    def tools(self, path):
        return {"list_endpoints"}, ""

    def datasets(self):
        return {"air-data"} - self.missing, ""

    def realm(self, name):
        return name == "city", "200"

    def client_answers(self, realm, clients):
        known = {"portal-api", "gone"} - self.missing
        return {c: (400, "invalid_request") if c in known else (401, "invalid_client") for c in clients}


def run_fixture(tmp_path, missing=(), renames=()):
    export = tmp_path / "export"
    write_export(export)
    import prodnames
    prod = prodnames.load(export)
    res = Results("names", "http://localhost")
    a = type("A", (), {"repo": suite.REPO, "state": None})()
    prod_tools = {"public": {"/api/mcp": {"tools": ["list_endpoints"]}, "/api/mcp/p/open": {"tools": ["list_endpoints"]}},
                  "source": {"tools": {"gateway": ["create_entity"], "anonymous": ["list_endpoints"],
                                       "broker": ["query_entities"]}}}
    extra = suite.run(a, prod, FakeJc(missing), res, list(renames), prod_tools, export)
    return res, extra


def test_the_whole_run_on_an_equal_jc_is_green_and_reports_the_extra_project(tmp_path):
    res, extra = run_fixture(tmp_path)
    fails = [r for r in res.rows if r["result"] == FAIL]
    assert fails == [], fails
    kinds = {r["id"] for r in res.rows}
    assert {"project", "space", "endpoint-name", "endpoint", "surface", "mcp-server", "tool", "dataset",
            "realm", "client", "connector"} <= kinds
    assert extra["project"] == ["extra-project"]
    assert any(r["id"] == "connector" and r["result"] == SKIP for r in res.rows)
    assert any(r["id"] == "tool" and "public reads only" in r["reason"] for r in res.rows)


def test_the_whole_run_fails_each_name_jc_lost(tmp_path):
    res, _ = run_fixture(tmp_path, missing={"p/air", "air-data", "gone", "/api/mcp/p/open"})
    failed = {(r["id"], r["target"]) for r in res.rows if r["result"] == FAIL}
    assert failed == {("space", "p/air"), ("dataset", "air-data"), ("client", "city/gone"),
                      ("mcp-server", "/api/mcp/p/open")}


def test_a_surface_that_answers_for_an_unknown_slug_proves_nothing(tmp_path):
    class Lenient(FakeJc):
        def statuses(self, calls):
            return {c: 200 for c in calls}

    export = tmp_path / "export"
    write_export(export)
    import prodnames
    res = Results("names", "http://localhost")
    a = type("A", (), {"repo": suite.REPO, "state": None})()
    suite.run(a, prodnames.load(export), Lenient(), res, [], None, export)
    failed = {(r["id"], r["target"]) for r in res.rows if r["result"] == FAIL}
    assert ("surface.control", "ngsi-ld") in failed
    # The Endpoint is public: answering like an unknown slug is no answer.
    assert ("surface", "/api/endpoint/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ngsi-ld/v1/types") in failed
    assert ("endpoint", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa") in failed


def test_a_closed_endpoints_mcp_door_that_answers_any_slug_401_is_a_skip_row(tmp_path):
    export = tmp_path / "export"
    write_export(export)
    f = export / "gitea/files/joinedcontext/p/spaces/air/endpoints/open.yaml"
    f.write_text(f.read_text().replace("audience: public", "audience: organization"))

    class Closed(FakeJc):
        def statuses(self, calls):
            return {(m, p): 404 if "smoke-names-" in p and m == "GET" else 401 if p.startswith("/api/endpoint/")
                    else 200 for m, p in calls}

    import prodnames
    res = Results("names", "http://localhost")
    a = type("A", (), {"repo": suite.REPO, "state": None})()
    suite.run(a, prodnames.load(export), Closed(), res, [], None, export)
    r = {(x["id"], x["target"]): x["result"] for x in res.rows}
    assert r[("surface", "/api/endpoint/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/mcp")] == SKIP
    assert r[("surface", "/api/endpoint/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ngsi-ld/v1/types")] == PASS
    assert r[("endpoint", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")] == PASS


def test_an_unknown_client_id_and_a_known_one_are_told_apart_by_the_realms_refusal(tmp_path):
    res, _ = run_fixture(tmp_path)
    r = {(x["id"], x["target"]): x["result"] for x in res.rows}
    assert r[("client.control", "city")] == PASS and r[("client", "city/portal-api")] == PASS


def test_anonymous_names_the_portal_listing_kinds_skipped_and_still_reads_the_public_ones(tmp_path):
    """Live mode: no signed-in administrator, so the Portal-listing kinds are named skips; the
    anonymous kinds (endpoint, surface, mcp-server, dataset, realm, client) are still read."""
    export = tmp_path / "export"
    write_export(export)
    import prodnames
    res = Results("names", "http://localhost")
    a = type("A", (), {"repo": suite.REPO, "state": None, "anonymous": True})()
    suite.run(a, prodnames.load(export), FakeJc(), res, [], None, export)
    by = {}
    for r in res.rows:
        by.setdefault(r["id"], set()).add(r["result"])
    for kind in ("project", "space", "endpoint-name", "app", "dashboard"):
        assert by[kind] == {SKIP}, (kind, by[kind])
    for kind in ("endpoint", "surface", "mcp-server", "dataset", "realm", "client"):
        assert by[kind] == {PASS}, (kind, by[kind])
