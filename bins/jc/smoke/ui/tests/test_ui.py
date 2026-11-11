"""Unit tests of the `ui` smoke suite: python3 -m pytest bins/jc/smoke/ui/tests"""

import json
import re
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import checks  # noqa: E402
import suite  # noqa: E402
import walk  # noqa: E402
from common import Results  # noqa: E402

GOOD = """<html lang="en"><head><title>T</title></head><body><div id="root"><main>
<h1>Spaces</h1><p>The spaces of this project and what they hold.</p>
<table><thead><tr><th>ID</th><th>Name</th></tr></thead>
<tbody><tr><td>urn:ngsi-ld:Space:1</td><td>Parking</td></tr></tbody></table>
</main></div></body></html>"""

BAD = """<html lang="en"><head><title>T</title></head><body><div id="root"><main>
<h1>Spaces</h1>
<div data-empty-state><p>Nothing here.</p></div>
<table><thead><tr><th>Owner</th></tr></thead><tbody><tr><td>urn:ngsi-ld:Person:7</td></tr></tbody></table>
<p>{"kind": "Space"}</p><button>urn:ngsi-ld:Space:1</button><button>Delete</button>
</main></div></body></html>"""


@pytest.fixture(scope="module")
def page():
    sync = pytest.importorskip("playwright.sync_api")
    with sync.sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage"])
        yield browser.new_page()
        browser.close()


def judge(page, html):
    page.set_content(html)
    page.add_script_tag(content=checks.script())
    out = page.evaluate("o => window.__jcSmoke.pageFindings(o)", {"namespaces": checks.namespaces(), "layout": True})
    return out + page.evaluate(checks.TABLE_URN) + page.evaluate("() => window.__jcSmoke.viewerWriteFindings(true)")


def test_the_portal_checks_load_from_the_portal_source_and_pass_a_good_page(page):
    assert judge(page, GOOD) == []


def test_the_checks_name_every_defect_of_a_bad_page(page):
    found = {f.split(":")[0] for f in judge(page, BAD)}
    assert {"purpose-line", "empty-state-next-step", "raw-json", "bare-urn", "table-urn", "viewer-write"} <= found


def test_a_signed_in_browser_on_a_127_origin_reads_the_api_with_its_session_cookie(page):
    """jc-ip-signin 2026-11-10: on the rehearsal's http://127.0.0.1 origin Chromium keeps and sends
    the Portal's __Host- session cookie, but Playwright's request client did not, so every sign-in
    row read /api/v1/auth/me 401. browser_get sends what the browser would."""
    import http.server
    import threading

    class Portal(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            if self.path == "/login":
                self.send_header("Set-Cookie", "__Host-jc_session=s1; Path=/; Secure; HttpOnly; SameSite=Lax")
            self.end_headers()
            self.wfile.write((self.headers.get("Cookie") or "").encode())

        def log_message(self, *_):
            pass

    srv = http.server.HTTPServer(("127.0.0.1", 0), Portal)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_port}"
    try:
        page.goto(base + "/login")
        assert page.evaluate("fetch('/me').then(r => r.text())") == "__Host-jc_session=s1"
        assert suite.browser_get(page, base + "/api/v1/auth/me").text() == "__Host-jc_session=s1"
        assert suite.browser_get(page, base + "/me", headers={"Cookie": "own=1"}).text() == "own=1"
    finally:
        srv.shutdown()
        page.context.clear_cookies()


def test_the_stripped_source_keeps_no_typescript():
    js = checks.script()
    assert not re.search(r"\bas HTML|: string\b|: Element\b|<string>", js)
    for name in checks.FUNCTIONS:
        assert f"function {name}(" in js


def ts_list(name: str) -> list[str]:
    src = (checks.UI / "e2e" / "live" / "walk-routes.ts").read_text()
    body = re.search(rf"const {name} = \[(.*?)\];", src, re.S).group(1)
    return re.findall(r'"([^"]+)"', body)


@pytest.mark.parametrize("name", ["FORMS", "SECTIONS", "ORGANIZATION_TABS", "SETTINGS_TABS"])
def test_the_route_lists_follow_walk_routes_ts(name):
    assert getattr(walk, name) == ts_list(name)


def test_the_project_pages_follow_walk_routes_ts():
    src = (checks.UI / "e2e" / "live" / "walk-routes.ts").read_text()
    block = src[src.index('"home", "inbox"'):src.index("].map((page)")]
    assert walk.PROJECT_PAGES == re.findall(r'"([^"]+)"', block)


@pytest.mark.parametrize("who,status,url,ok", [
    ("viewer", 403, "http://x/api/v1/projects/p/policies", True),
    ("outsider", 403, "http://x/api/v1/projects/p/policies", True),
    ("admin", 403, "http://x/api/v1/projects/p/policies", False),
    ("admin", 404, "http://x/api/v1/projects/p/drafts/Space/a", True),
    ("admin", 404, "http://x/api/v1/projects/p/spaces/a", False),
    ("admin", 410, "http://x/api/v1/anything", True),
    ("outsider", 404, "http://x/api/v1/projects/p/spaces", True),
    ("anonymous", 401, "http://x/api/v1/auth/me", True),
    ("viewer", 401, "http://x/api/v1/auth/me", False),
    ("viewer", 404, "http://x/api/v1/projects/p/spaces", False),
])
def test_expected_4xx_follows_the_walker(who, status, url, ok):
    assert suite.expected_4xx(who, status, "GET", url) is ok


def test_the_judge_passes_only_scores_of_four_and_up_without_a_high_finding(tmp_path):
    shots = [{"page": f"portal:admin:/{n}", "width": 1440} for n in "abcd"]
    (tmp_path / "j.json").write_text(json.dumps([
        {"page": "portal:admin:/a", "width": 1440, "scores": {"layout": 5, "phone": 4}, "findings": []},
        {"page": "portal:admin:/b", "width": 1440, "scores": {"layout": 3}, "findings": []},
        {"page": "portal:admin:/c", "width": 1440, "scores": {"layout": 5},
         "findings": [{"severity": "high", "text": "raw JSON in the table"}]},
    ]))
    res = Results("ui", "http://localhost")
    suite.fold_judge(res, shots, str(tmp_path / "j.json"))
    got = {r["id"].rsplit("/", 1)[1]: r["result"] for r in res.rows}
    assert got == {"a": "pass", "b": "fail", "c": "fail", "d": "skip"}


def test_the_shako_check_ids_carry_no_credential_shape():
    # The logs suite scans every results file; a check id reading `...-token:/git/...` was
    # flagged as a password field in the 2026-11-09 gate.
    sys.path.insert(0, str(HERE.parent.parent / "logs"))
    import logscan
    for rid in (f"judge:1440:shako:{suite.SHAKO_PERSONA}:/git/jc/config",
                f"shako.page:{suite.SHAKO_PERSONA}:1440:/git/"):
        assert logscan.credential_shapes(rid) == [], rid


def test_the_persona_admin_calls_the_realm_it_is_given():
    import personas

    class H:
        seen = []

        def call(self, m, path, *a, **k):
            self.seen.append(path)
            return 200, {}, b"[]"

    h = H()
    personas.Admin(h, "t", "smartcity").groups()
    assert h.seen == ["/id/admin/realms/smartcity/groups?briefRepresentation=false&max=1000"]


def test_the_persona_admin_mints_its_token_in_the_realm_and_names_a_refusal():
    import personas

    class H:
        def __init__(self, st, raw):
            self.st, self.raw, self.seen = st, raw, []

        def call(self, m, path, *a, headers=None, body=None, **k):
            self.seen.append((m, path, headers, body))
            return self.st, {}, self.raw

    h = H(200, b'{"access_token": "jwt"}')
    assert personas.admin_token(h, "s3", "smartcity") == "jwt"
    m, path, headers, body = h.seen[0]
    assert (m, path) == ("POST", "/id/realms/smartcity/protocol/openid-connect/token")
    assert headers["Content-Type"] == "application/x-www-form-urlencoded"
    assert b"client_id=portal-reconciler" in body and b"grant_type=client_credentials" in body
    with pytest.raises(RuntimeError, match="portal-reconciler token: 401"):
        personas.admin_token(H(401, b"{}"), "s3")


def test_the_daiya_page_is_opened_in_the_dashboards_own_organization_as_the_portal_links_it():
    class R:
        def __init__(self, status, body):
            self.status, self.body = status, body

        def json(self):
            return self.body

    class Page:
        def __init__(self, r):
            self.r, self.seen = r, []
            self.context = type("C", (), {"cookies": lambda _self: []})()
            self.request = self

        def get(self, url, **k):
            self.seen.append(url)
            return self.r

    p = Page(R(200, {"uid": "project-helsinki", "orgId": 2}))
    assert suite.daiya_page(p, "http://127.0.0.1:1", "helsinki", "project-helsinki") == "/dashboards/d/project-helsinki?orgId=2"
    assert p.seen == ["http://127.0.0.1:1/api/v1/projects/helsinki/grafana/dashboards/project-helsinki"]
    assert suite.daiya_page(Page(R(404, {})), "http://127.0.0.1:1", "helsinki", "x") == "/dashboards/d/x"


def test_the_runner_never_hands_ui_or_charts_apps_jcs_admin_listener_token():
    # The runner's --admin-token-file is jc's admin listener token (logs reads it with --admin-url);
    # the realm refuses it as a bearer (smoke run 42945870109: persona rows "invalid admin bearer token").
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "all"))
    import importlib.util
    spec = importlib.util.spec_from_file_location("all_suite", Path(__file__).resolve().parents[2] / "all/suite.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    for name in ("ui", "charts-apps"):
        assert "--admin-token-file" not in runner.declared(name), name


def test_a_page_whose_only_finding_is_the_assistants_5xx_is_a_kept_component_skip(tmp_path):
    assert suite.kept_component("http-5xx: 503 GET /api/v1/projects/bbsk/knowledge/sources")
    assert suite.kept_component("http-5xx: 503 GET /api/v1/projects/bbsk/knowledge?x=1")
    assert not suite.kept_component("http-5xx: 503 POST /api/v1/projects/bbsk/endpoints/a/preview")
    assert not suite.kept_component("http: 404 GET /api/v1/projects/bbsk/knowledge/sources")

    class Loc:
        first = property(lambda self: self)
        def wait_for(self, timeout=0): pass

    class Page:
        def __init__(self, extra):
            self.extra = extra
        def goto(self, url, **k):
            w.current += ["http-5xx: 503 GET /api/v1/projects/bbsk/knowledge/sources"] + self.extra
            return type("R", (), {"status": 200})()
        def wait_for_load_state(self, *a, **k): pass
        def locator(self, *a): return Loc()
        def set_viewport_size(self, *a): pass
        def wait_for_timeout(self, *a): pass

    def rows(extra):
        global w
        res = Results("ui", "http://x")
        w = object.__new__(suite.Walker)
        w.a = type("A", (), {"widths": [1440]})()
        w.res, w.out, w.shots, w.current, w.allow, w.base = res, tmp_path, [], [], [], "http://x"
        w.dom = lambda page, **k: []
        w.h1 = lambda page: "Knowledge"
        w.screenshot = lambda page, who, width, address: tmp_path / "s.png"
        suite.Walker.judge(w, Page(extra), "admin", "portal", "/projects/$p/knowledge",
                           "/projects/bbsk/knowledge", portal=False, viewer=False)
        return res.rows
    only = rows([])
    assert [r["result"] for r in only] == ["skip"] and only[0]["reason"].startswith("kept component: assistant")
    # Any other finding on the page keeps the row a fail, naming only that finding.
    mixed = rows(["console: boom"])
    assert [r["result"] for r in mixed] == ["fail"] and "knowledge" not in mixed[0]["actual"]


# Gate 2026-11-09T192034Z: the explorer of helsinki-transport opened on Alert after the space's
# retention (Alert current: PT1H) had removed the rehearsal's 28 Alerts, whose writer cannot write
# there; the grid showed its empty state and the check waited 45 s for rows that could not come.
EXPLORE = """<html><body><p id="status"></p><div id="g"></div><script>
const t = (location.hash.match(/type=([^&]+)/) || [])[1];
document.getElementById("g").innerHTML = DRAWN[t]
  ? "<table><tbody><tr><td>" + t + " 1</td></tr></tbody></table>"
  : "<table><tbody></tbody></table><div class='jc-grid-empty'>No entities here.</div>";
</script></body></html>"""


def viz_server(held: dict, drawn: dict):
    import http.server
    import threading

    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            path, _, query = self.path.partition("?")
            headers, body = {"Content-Type": "text/html"}, ""
            if path == "/opendata/dataset/x":
                # Another origin than the page, as an opaque sandboxed frame is: its location is not the page's to read.
                body = f'<iframe class="jc-explorer-frame" src="http://localhost:{self.server.server_port}/viz/explore.html#slug1&inline&type=Alert"></iframe>'
            elif path in suite.SANDBOX_PAGES:
                headers["Content-Security-Policy"] = "sandbox allow-scripts"
                body = EXPLORE.replace("DRAWN", json.dumps(drawn))
            elif path == "/api/endpoint/slug1/ngsi-ld/v1/types":
                headers["Content-Type"], body = "application/json", json.dumps({"typeList": sorted(held) + ["Alert"]})
            elif path == "/api/endpoint/slug1/ngsi-ld/v1/entities":
                t = re.search(r"type=([^&]+)", query).group(1)
                headers["Content-Type"], headers["NGSILD-Results-Count"], body = "application/json", str(held.get(t, 0)), "[]"
            self.send_response(200 if body else 404)
            for k, v in headers.items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body.encode())

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


@pytest.mark.parametrize("held, drawn, ok, said", [
    ({"Alert": 2}, {"Alert": 1}, True, "1 rows"),
    ({"GtfsStop": 3}, {"GtfsStop": 1}, True, "1 rows of GtfsStop; the opened type Alert holds no entity at the Endpoint"),
    ({"GtfsStop": 3, "Alert": 2}, {"GtfsStop": 1}, False, "the grid shows no Alert while the Endpoint holds 2"),
    ({}, {}, False, "the Endpoint holds no entity of Alert or any other type"),
])
def test_an_explorer_opened_on_an_empty_type_still_owes_rows_of_a_type_the_endpoint_holds(page, held, drawn, ok, said):
    srv = viz_server(held, drawn)
    ctx = page.context.browser.new_context()
    try:
        w = object.__new__(suite.Walker)
        w.res, w.base = Results("ui", "http://x"), f"http://127.0.0.1:{srv.server_port}"
        suite.viz_sandbox(w, ctx, ctx.new_page(), "/opendata/dataset/x")
        row = next(r for r in w.res.rows if r["id"] == "viz.grid")
        assert (row["result"] == "pass") == ok, row["actual"]
        assert said in row["actual"], row["actual"]
    finally:
        ctx.close()
        srv.shutdown()
