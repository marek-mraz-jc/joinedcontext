"""Selftest of the catalogue suite over a fixture: a small prod capture against a stub dataroom on
loopback that answers CKAN actions and a paged DCAT feed; a missing dataset, a wrong resource count,
a lost alias and a stub answering 200 to everything must fail.

    python3 -m pytest bins/jc/smoke/catalogue/tests -q
"""

import importlib.util
import json
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent
SUITE_DIR = HERE.parent
sys.path.insert(0, str(SUITE_DIR))
sys.path.insert(0, str(SUITE_DIR.parent))

_spec = importlib.util.spec_from_file_location("smoke_catalogue_suite", SUITE_DIR / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)
import fetch_prod_ckan

PROD = {
    "fetchedAt": "2026-11-09T00:00:00Z", "organizations": ["praha", "zilina"],
    "packageList": ["air", "catalog", "ghost", "trees"], "listedNotFound": ["ghost"], "searchCount": 3,
    "perOrganization": {"praha": 2, "zilina": 1},
    "dcat": {"pages": 2, "totalItems": 3, "catalogNodes": 2, "datasetNodes": 3},
    "datasets": {"air": {"organization": "praha", "resources": 2, "dcatDistributions": 2},
                 "trees": {"organization": "praha", "resources": 1, "dcatDistributions": 1},
                 "catalog": {"organization": "zilina", "resources": 3, "dcatDistributions": 3}},
}
RENAMES = """| kind | old | new | why | how old clients keep working |
|---|---|---|---|---|
| dataset | catalog | zilina-catalog | one name, three Endpoints | dataroom answers `catalog` as an alias |
"""


def jc_store():
    return {"air": ("praha", 2), "trees": ("praha", 1), "zilina-catalog": ("zilina", 3)}


def dataroom(store, aliases=None, compact=False):
    aliases = {"catalog": "zilina-catalog"} if aliases is None else aliases

    def pkg(name):
        org, n = store[name]
        return {"id": f"id-{name}", "name": name, "organization": {"name": org},
                "resources": [{"url": f"r{i}"} for i in range(n)]}

    def answer(path):
        u = urllib.parse.urlparse(path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        ok = lambda r: (200, {"success": True, "result": r})
        if u.path == "/opendata/api/3/action/organization_list":
            return ok(sorted({o for o, _ in store.values()}))
        if u.path == "/opendata/api/3/action/package_list":
            return ok(sorted(store))
        if u.path == "/opendata/api/3/action/package_show":
            name = aliases.get(q.get("id"), q.get("id"))
            return ok(pkg(name)) if name in store else (404, {"success": False, "error": {"__type": "Not Found Error"}})
        if u.path == "/opendata/api/3/action/package_search":
            names = sorted(store)
            if "fq" in q:
                org = q["fq"].split('"')[1]
                names = [n for n in names if store[n][0] == org]
            start, rows = int(q.get("start", 0)), int(q.get("rows", 10))
            return ok({"count": len(names), "results": [pkg(n) for n in names[start:start + rows]]})
        if u.path == "/opendata/catalog.jsonld":
            page = int(q.get("page", 1))
            names = sorted(store)[(page - 1) * 2:page * 2]
            base = "http://x/opendata"
            if compact:
                return 200, {"@context": {"dcat": "http://www.w3.org/ns/dcat#", "hydra": "http://www.w3.org/ns/hydra/core#"},
                             "@graph": [{"@id": base, "@type": "dcat:Catalog",
                                         "dcat:dataset": [{"@id": f"{base}/dataset/{n}", "@type": "dcat:Dataset",
                                                           "dcat:distribution": [{"@id": f"d{i}"} for i in range(store[n][1])]}
                                                          for n in names]},
                                        {"@id": f"{base}/catalog.jsonld?page={page}", "@type": "hydra:PagedCollection",
                                         "hydra:totalItems": len(store), "hydra:lastPage": f"{base}/catalog.jsonld?page=2"}]}
            d, h = "http://www.w3.org/ns/dcat#", "http://www.w3.org/ns/hydra/core#"
            return 200, [{"@id": base, "@type": [f"{d}Catalog"]}] + [
                {"@id": f"{base}/dataset/id-{n}", "@type": [f"{d}Dataset"],
                 f"{d}distribution": [{"@id": f"d{i}"} for i in range(store[n][1])]} for n in names] + [
                {"@id": "p", "@type": [f"{h}PagedCollection"], f"{h}totalItems": [{"@value": len(store)}],
                 f"{h}lastPage": [{"@value": f"{base}/catalog.jsonld?page=2"}]}]
        return 404, {"error": "no"}
    return answer


def serve(answer):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            st, body = answer(self.path)
            self.send_response(st)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body).encode())

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def run(tmp_path, answer, export_env="prod"):
    (tmp_path / "prod.json").write_text(json.dumps(PROD))
    (tmp_path / "RENAMES.md").write_text(RENAMES)
    export = tmp_path / "export"
    export.mkdir(exist_ok=True)
    (export / "manifest.json").write_text(json.dumps({"env": export_env}))
    srv = serve(answer)
    try:
        out = tmp_path / "out"
        code = suite.main(["--base-url", f"http://127.0.0.1:{srv.server_port}", "--out", str(out),
                           "--prod", str(tmp_path / "prod.json"), "--renames", str(tmp_path / "RENAMES.md"),
                           "--export", str(export)])
        rows = {r["id"]: r for r in json.loads((out / "results.json").read_text())["checks"]}
        return code, rows
    finally:
        srv.shutdown()


def fails(rows):
    return sorted(i for i, r in rows.items() if r["result"] == "fail")


def test_a_dataroom_equal_to_prod_passes_with_the_rename_and_the_ghost_name(tmp_path):
    for compact in (False, True):
        code, rows = run(tmp_path, dataroom(jc_store(), compact=compact))
        assert fails(rows) == [] and code == 0, (compact, fails(rows))
        assert rows["rename.catalog"]["result"] == "pass"
        assert "404" in rows["package_list.ghost"]["actual"]
        assert rows["dcat.total"]["result"] == "pass"


def test_a_missing_dataset_a_wrong_count_and_a_lost_alias_fail(tmp_path):
    store = jc_store()
    del store["trees"]
    store["air"] = ("praha", 5)
    code, rows = run(tmp_path, dataroom(store, aliases={}))
    assert code == 1
    assert {"package_list.trees", "package_show.trees", "package_show.air", "package_search.praha",
            "rename.catalog", "dcat.dataset.trees", "dcat.dataset.air", "dcat.total"} <= set(fails(rows))
    assert "package_show.catalog" not in fails(rows)


def test_a_stub_answering_200_to_everything_fails(tmp_path):
    code, rows = run(tmp_path, lambda path: (200, {"success": True, "result": {"name": "x"}}))
    assert code == 1 and "control.package_show-unknown" in fails(rows)


def test_a_synthetic_export_skips_the_comparison_by_name(tmp_path):
    code, rows = run(tmp_path, dataroom(jc_store()), export_env="dev")
    assert rows["package_show"]["result"] == "skip" and "not prod's" in rows["package_show"]["reason"]
    assert rows["dcat.catalog"]["result"] == "pass" and code == 0


def test_the_committed_prod_fixture_holds_no_ids_or_timestamps():
    f = max((SUITE_DIR / "fixtures").glob("prod-ckan-*.json"))
    doc = json.loads(f.read_text())
    assert doc["datasets"] and doc["organizations"]
    for d in doc["datasets"].values():
        assert set(d) <= {"organization", "resources", "dcatDistributions", "inSearch"}


def test_expand_reads_compact_and_expanded_feeds_alike():
    compact = {"@context": {"dcat": "http://www.w3.org/ns/dcat#"},
               "@graph": [{"@id": "c", "@type": "dcat:Catalog",
                           "dcat:dataset": {"@id": "u/dataset/a", "@type": "dcat:Dataset", "dcat:distribution": [{"@id": "1"}]}}]}
    nodes = fetch_prod_ckan.expand(compact)
    assert fetch_prod_ckan.distributions_by_package(nodes) == {"a": 1}
