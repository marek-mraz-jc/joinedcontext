"""Unit checks of the charts-apps suite's pure parts: python3 -m pytest bins/jc/smoke/charts-apps/tests"""

import datetime as dt
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))
sys.path.append(str(HERE.parent.parent / "endpoints"))  # after charts-apps: endpoints has a suite.py too

import inventory as inv
import suite

PANEL = {"id": 2, "type": "timeseries", "title": "pm25", "datasource": {"type": "ngsild-grafana-datasource", "uid": "aq"},
         "targets": [{"queryType": "temporal", "entityType": "AirQualityObserved", "attributes": ["pm25"]}]}


def write(d: Path, name: str, doc: dict):
    (d / name).write_text(json.dumps(doc))


def test_both_file_shapes_map_to_projects(tmp_path):
    write(tmp_path, "helsinki.dashboard.json", {"dashboard": {"uid": "project-helsinki", "title": "H", "panels": [PANEL]}})
    write(tmp_path, "helsinki.datasource.aq.json", {"uid": "aq", "type": "ngsild-grafana-datasource",
                                                    "url": "http://gw/api/endpoint/" + "a" * 32})
    write(tmp_path, "portal.json", {"uid": "jc-portal", "title": "Portal", "panels": [{"id": 1, "type": "stat",
                                    "datasource": {"type": "prometheus", "uid": "prometheus"}, "targets": [{"expr": "up"}]}]})
    exp = tmp_path / "exp"
    exp.mkdir()
    write(exp, "org3-project-helsinki.json", {"uid": "project-helsinki", "title": "H", "panels": [PANEL]})
    write(exp, "org3-pv-1.json", {"uid": "pv-1", "title": "view", "panels": [PANEL]})
    write(exp, "org9-pv-2.json", {"uid": "pv-2", "title": "lost", "panels": []})
    write(exp, "org1-jc-portal.json", {"uid": "jc-portal", "title": "Portal", "panels": []})
    dash, ds = inv.load_dashboards([exp, tmp_path])
    keys = {(d.project, d.uid): d for d in dash}
    assert set(keys) == {("helsinki", "project-helsinki"), ("helsinki", "pv-1"), ("org", "jc-portal"), ("source-org-9", "pv-2")}
    assert len(keys[("helsinki", "project-helsinki")].sources) == 2  # deduped by project and uid
    assert keys[("source-org-9", "pv-2")].problem
    assert ds[0].slug == "a" * 32 and ds[0].project == "helsinki"
    assert sorted(keys[("org", "jc-portal")].sources) == ["org1-jc-portal.json", "portal.json"]


def test_expectation_follows_the_export():
    pn = inv.panels_of({"panels": [PANEL]})[0]
    since = dt.datetime(2026, 11, 8, 0, 0, tzinfo=dt.UTC)
    temporal = {("t", "AirQualityObserved", "pm25"): ["2026-11-07T23:00:00Z", "2026-11-08T01:00:00Z"]}
    assert inv.expect_panel(pn, "t", {}, temporal, since)[0] == "data"
    assert inv.expect_panel(pn, "t", {}, temporal, since + dt.timedelta(hours=2))[0] == "empty"
    assert inv.expect_panel(pn, None, {}, temporal, since)[0] == "unknown"
    table = inv.Panel(3, "t", "table", "x", "aq", [{"queryType": "entity", "entityType": "AirQualityObserved"}])
    assert inv.expect_panel(table, "t", {"t": {"AirQualityObserved": {"count": 4}}}, {}, since)[0] == "data"
    assert inv.expect_panel(table, "t", {"t": {}}, {}, since)[0] == "empty"


def test_judge_never_passes_a_missing_answer():
    frames = [{"fields": [{"name": "time", "values": [1, 2]}, {"name": "v", "values": [3, 4]}]}]
    assert suite.frame_rows(frames) == 2
    assert suite.judge(200, frames, "", "data", "")[0] == suite.PASS
    assert suite.judge(200, [], "", "data", "x")[0] == suite.FAIL
    assert suite.judge(200, [], "", "empty", "x")[0] == suite.PASS
    assert suite.judge(200, [], "", "unknown", "x")[0] == suite.SKIP
    assert suite.judge(502, frames, "", "data", "")[0] == suite.FAIL
    assert suite.judge(200, frames, "cr.ssrf_blocked", "data", "")[0] == suite.FAIL
    assert suite.daiya_frames({"frameSets": [{"refId": "A", "frames": frames}]}) == (frames, "")
    assert suite.daiya_frames({"message": "no"})[1] == "no"


def test_a_closed_endpoints_panel_without_a_person_passes_only_on_the_gateways_refusal():
    frame = [{"data": {"values": [[1], [2]]}}]
    refused = "outbound datasource error: upstream status 401: unauthorized"
    assert suite.judge_closed(502, [], refused)[0] == suite.PASS
    assert suite.judge_closed(502, [], "upstream status 403: forbidden")[0] == suite.PASS
    assert suite.judge_closed(200, frame, "")[0] == suite.FAIL  # a row is a leak
    assert suite.judge_closed(502, [], "upstream status 500: boom")[0] == suite.FAIL
    assert suite.judge_closed(200, [], "")[0] == suite.FAIL


def test_a_closed_panel_read_as_the_granted_reader_passes_only_on_rows():
    frames = [{"fields": [{"name": "name", "values": ["a", "b"]}]}]
    assert suite.judge_reader(200, frames, "") == (suite.PASS, "200, 1 frames, 2 rows", "")
    assert suite.judge_reader(200, [], "")[0] == suite.FAIL  # 200 with zero rows
    assert suite.judge_reader(200, [{"fields": []}], "")[0] == suite.FAIL  # a frame without rows
    assert suite.judge_reader(500, frames, "")[0] == suite.FAIL
    assert suite.judge_reader(200, [], "outbound datasource error: upstream status 403: no policy")[0] == suite.FAIL
    r, _, why = suite.judge_reader(200, [], "this panel is read with your own access token, which this session "
                                            "does not carry")
    assert r == suite.FAIL and "JC_TRUST_EDGE_TOKEN" in why
    assert suite.panel_answer(200, {"frames": frames}) == (frames, "")
    assert suite.panel_answer(200, {"accessDenied": "You cannot read"}) == ([], "You cannot read")
    assert suite.panel_answer(500, {"detail": "boom"}) == ([], "boom")


def test_a_closed_panel_read_by_a_person_no_policy_names_passes_only_on_the_gateways_refusal():
    frames = [{"fields": [{"name": "name", "values": ["a"]}]}]
    for status in (403, 404):
        refused = f"outbound datasource error: upstream status {status}: refused"
        assert suite.judge_refused(200, [], refused)[0] == suite.PASS
    r, _, why = suite.judge_refused(200, frames, "")
    assert r == suite.FAIL and "LEAK" in why  # any row is a leak
    assert suite.judge_refused(200, [{"fields": []}], "upstream status 403")[0] == suite.FAIL  # a frame too
    assert suite.judge_refused(200, [], suite.PERSON_REFUSED)[0] == suite.PASS  # the refusal state
    assert suite.judge_refused(200, frames, suite.PERSON_REFUSED)[0] == suite.FAIL
    assert suite.judge_refused(200, [], "")[0] == suite.FAIL  # silence is no refusal
    assert suite.judge_refused(500, [], "upstream status 403")[0] == suite.FAIL
    assert suite.judge_refused(200, [], "upstream status 500: boom")[0] == suite.FAIL
    # smoke 42975117838: the gateway's 404 reached the page in the Portal's own words
    silent = "the panel's datasource did not answer; try again in a minute, or ask the dashboard's owner"
    assert suite.judge_refused(200, [], silent, source_answered=True)[0] == suite.PASS
    assert suite.judge_refused(200, [], silent)[0] == suite.FAIL  # a source down for everyone
    assert suite.judge_refused(200, frames, silent, source_answered=True)[0] == suite.FAIL


def test_a_closed_panel_gives_one_row_per_person_with_the_answer_it_got():
    """Run 42969262429 crashed the suite here: each judge's verdict lands in `result`, its text in
    `actual`, and a person without an edge token is a failing row, never a crash."""
    from common import Results

    rows = {"reader-token": {"frames": [{"fields": [{"name": "name", "values": ["a", "b"]}]}]},
            "person-token": {"error": "outbound datasource error: upstream status 404: not found"}}

    class Portal:
        def json(self, method, path, headers=None):
            return 200, rows[headers["X-Access-Token"]]

    tokens = {suite.READER: ("reader-token", ""), suite.PERSON: ("person-token", "")}
    res = Results("charts-apps", "http://127.0.0.1:1")
    suite.read_closed(Portal(), res, "/api/v1/projects/p/grafana/dashboards/d/panels/4?from=1", "t", tokens.get)
    assert [(r["id"], r["result"], r["actual"]) for r in res.rows] == [
        ("portal.panel", suite.PASS, "200, 1 frames, 2 rows"),
        ("portal.panel.refused", suite.PASS,
         "200, 0 frames, 0 rows, error: outbound datasource error: upstream status 404: not found")]
    # the Portal's own words for the refusal pass only beside the reader's rows
    rows["person-token"] = {"error": suite.SOURCE_SILENT + "; try again in a minute"}
    for reader, verdict in ((rows["reader-token"], suite.PASS), ({"frames": []}, suite.FAIL)):
        rows["reader-token"] = reader
        res = Results("charts-apps", "http://127.0.0.1:1")
        suite.read_closed(Portal(), res, "/p?x", "t", tokens.get)
        assert res.rows[1]["result"] == verdict, res.rows
    res = Results("charts-apps", "http://127.0.0.1:1")
    suite.read_closed(Portal(), res, "/p?x", "t", lambda who: (None, "login of x at edge: 400"))
    assert [(r["result"], r["reason"]) for r in res.rows] == [(suite.FAIL, "no edge token: login of x at edge: 400")] * 2


def test_apps_from_manifests(tmp_path):
    d = tmp_path / "joinedcontext" / "helsinki" / "apps" / "air"
    d.mkdir(parents=True)
    (d / "app.yaml").write_text("apiVersion: joinedcontext.com/v1alpha1\nkind: App\nmetadata: {name: air, namespace: helsinki}\n"
                                "spec:\n  kind: ui-rust\n  lifecycle: published\n  source:\n    git:\n"
                                "      url: https://x/git/joinedcontext-apps/helsinki_air.git\n")
    (a,) = inv.load_apps(tmp_path)
    assert (a.project, a.name, a.kind, a.git) == ("helsinki", "air", "ui-rust", "joinedcontext-apps/helsinki_air")


def test_temporal_index_keys_short_names(tmp_path):
    (tmp_path / "antares").mkdir()
    iri = "https://uri.etsi.org/ngsi-ld/default-context/"
    (tmp_path / "antares" / "temporal.jsonl").write_text(json.dumps(
        {"tenant": "t", "id": "x", "type": iri + "AirQualityObserved", "attr": iri + "pm25",
         "instance": {"observedAt": "2026-11-08T01:00:00Z"}}) + "\n")
    assert inv.temporal_index(tmp_path, {"AirQualityObserved"}) == {("t", "AirQualityObserved", "pm25"): ["2026-11-08T01:00:00Z"]}


def test_the_session_helpers_call_the_realm_they_are_given(monkeypatch):
    import pytest
    import session
    seen = []
    monkeypatch.setattr(session.Session, "json", lambda self, m, path, *a, **k: seen.append(path) or (401, None))
    s = session.Session("http://127.0.0.1:9")
    with pytest.raises(RuntimeError, match="401"):
        session.admin_token(s, "x", "smartcity")
    with pytest.raises(RuntimeError, match="401"):
        session.make_person(s, "t", realm="smartcity")
    assert seen == ["/id/realms/smartcity/protocol/openid-connect/token", "/id/admin/realms/smartcity/users"]


def test_the_realm_reads_relative_secret_names_under_the_kits_secrets_dir(tmp_path, monkeypatch):
    # The kit writes $(file:NAME) relative to <home>/secrets/realm (81650d62): read from any cwd.
    import session
    (tmp_path / "state" / "realms").mkdir(parents=True)
    (tmp_path / "secrets" / "realm").mkdir(parents=True)
    (tmp_path / "secrets" / "realm" / "app-a").write_text("s3\n")
    (tmp_path / "secrets" / "realm" / "tester").write_text("pw\n")
    (tmp_path / "state" / "realms" / "smartcity.json").write_text(json.dumps({
        "realm": "smartcity",
        "clients": [{"clientId": "app-a", "secret": "$(file:app-a)", "redirectUris": ["https://a/*"]}],
        "users": [{"username": "rehearsal-tester",
                   "credentials": [{"type": "password", "value": "$(file:tester)"}]}]}))
    monkeypatch.chdir(tmp_path / "state")
    realm = session.Realm(tmp_path / "state")
    assert realm.client("app-a") == ("s3", "https://a/callback")
    assert realm.password("rehearsal-tester") == "pw"
    assert realm.client("app-b") == (None, None)


def test_the_app_host_is_rewritten_as_the_portals_edge_file_does():
    import shots

    base = "http://localhost:42141"
    host = shots.app_host(base, "alerts-desk")
    assert host == "http://alerts-desk.apps.localhost:42141"
    rw = shots.edge_rewrite
    assert rw(base, "alerts-desk", host + "/") == base + "/apps/alerts-desk/"
    assert rw(base, "alerts-desk", host + "/assets/i.css") == base + "/apps/alerts-desk/assets/i.css"
    assert rw(base, "alerts-desk", host + "/apps/alerts-desk/assets/i.css") == base + "/apps/alerts-desk/assets/i.css"
    assert rw(base, "alerts-desk", host + "/api/endpoint/s1/ngsi-ld/v1/entities?type=A") == \
        base + "/api/endpoint/s1/ngsi-ld/v1/entities?type=A"
    assert rw(base, "alerts-desk", host + "/apps/alerts-desk/api/endpoint/s1/x") == base + "/api/endpoint/s1/x"
    assert rw(base, "alerts-desk", "https://tiles.example/x.png") is None, "another host is left alone"


class FakeHost:
    """One host's answers by path, with every call it got."""

    def __init__(self, answers):
        self.answers, self.calls = answers, []

    def call(self, method, path, headers=None):
        self.calls.append((method, path, dict(headers or {})))
        return self.answers.get(path, (404, {}, b""))


def test_an_app_is_followed_from_the_main_host_to_its_own_host_and_judged_there():
    base, name = "http://localhost:42102", "praha-mapa"
    to = {"location": "http://praha-mapa.apps.localhost:42102/"}
    main = FakeHost({"/apps/praha-mapa/": (308, to, b"")})
    app = FakeHost({"/": (200, {"content-type": "text/html"}, b"<html>")})
    st, hdrs, actual = suite.open_app(main, app, base, name, {"Authorization": "Bearer t"})
    assert st == 200 and "html" in hdrs["content-type"] and actual.startswith("308 http://praha-mapa.apps.localhost:42102/")
    # the App's host is the base URL with its Host header (loopback, no DNS), the caller's token kept
    assert app.calls == [("GET", "/", {"Authorization": "Bearer t", "Host": "praha-mapa.apps.localhost:42102"})]
    # a refusal is the App host's own answer
    app = FakeHost({"/": (403, {"content-type": "application/problem+json"}, b"")})
    assert suite.open_app(main, app, base, name, {})[0] == 403


def test_a_redirect_off_the_apps_own_origin_is_never_followed_and_no_main_host_answer_passes():
    base, name = "http://localhost:42102", "praha-mapa"
    for loc in ("http://evil.example/", "http://praha-mapa.apps.evil.example:42102/", "http://localhost:42102/",
                "http://other.apps.localhost:42102/", "https://praha-mapa.apps.localhost:42102/",
                "http://praha-mapa.apps.localhost:9/", None):
        app = FakeHost({})
        main = FakeHost({"/apps/praha-mapa/": (308, {"location": loc} if loc else {}, b"")})
        st, _, actual = suite.open_app(main, app, base, name, {})
        assert st is None and not app.calls and "not followed" in actual, loc
    # prod answered 308 on the main host: neither its page nor its refusal there passes
    for answer, why in (((200, {"content-type": "text/html"}, b""), "REHEARSAL_APP_HOSTS=on"), ((403, {}, b""), "403")):
        app = FakeHost({})
        st, _, actual = suite.open_app(FakeHost({"/apps/praha-mapa/": answer}), app, base, name, {})
        assert st is None and not app.calls and why in actual


def test_a_wasm_server_passes_only_on_its_own_json_answer():
    j = {"content-type": "application/json"}
    assert suite.server_judge(200, j, b"null", False)[1]
    assert suite.server_judge(200, j, b'{"id": "s1", "roles": []}', True)[1]
    # The Portal's 503 for an App without a shard, its 401, a page, a problem and the wrong caller.
    p = {"content-type": "application/problem+json"}
    assert not suite.server_judge(503, p, b'{"status": 503}', False)[1]
    assert not suite.server_judge(401, p, b'{"status": 401}', True)[1]
    assert not suite.server_judge(200, {"content-type": "text/html"}, b"<html>", False)[1]
    assert not suite.server_judge(200, j, b'{"id": "s1"}', False)[1], "anonymous yet a caller"
    assert not suite.server_judge(200, j, b"null", True)[1], "signed in yet nobody"
    # The Portal's own page on an App host: jc mounts no App host, and the row says so.
    actual, ok = suite.server_judge(200, {"content-type": "text/html; charset=utf-8"}, b"<!doctype html>", False)
    assert not ok and "REHEARSAL_APP_HOSTS=on" in actual


def test_without_a_wasm_host_only_the_portals_designed_503_naming_the_app_passes():
    p = {"content-type": "application/problem+json"}
    body = b'{"status":503,"detail":"the server of App \'kpi-forecast\' does not run on this installation yet"}'
    assert suite.no_shard_judge(503, p, body, "kpi-forecast")[1]
    assert not suite.no_shard_judge(503, p, body, "news-topics")[1], "another App's 503"
    # The SPA (gate 5 rerun, no App host mount), the App's own 200, a 401 and a 503 naming the shard.
    actual, ok = suite.no_shard_judge(200, {"content-type": "text/html; charset=utf-8"}, b"<!doctype html>", "kpi-forecast")
    assert not ok and "REHEARSAL_APP_HOSTS=on" in actual
    assert not suite.no_shard_judge(200, {"content-type": "application/json"}, b"null", "kpi-forecast")[1]
    assert not suite.no_shard_judge(401, p, b'{"status":401}', "kpi-forecast")[1]
    leak = body[:-2] + b' (jc-wasm-host-0.wasm-host.svc.cluster.local)"}'
    assert not suite.no_shard_judge(503, p, leak, "kpi-forecast")[1]
    assert not suite.server_judge(0, {}, b"connection refused", False)[1]


def test_an_ip_literal_base_has_its_app_hosts_under_localhost():
    """The kit on http://127.0.0.1:<port> mounts App hosts as *.apps.localhost:<port>, and jc's 308
    names that host: an IP literal has no subdomains (gate rerun 2026-11-10T131506Z-g6: 48 rows
    expected banskabystrica-data.apps.127.0.0.1:42102)."""
    assert suite.shots_app_host("http://127.0.0.1:42102", "bike") == "http://bike.apps.localhost:42102"
    assert suite.shots_app_host("http://localhost:42102", "bike") == "http://bike.apps.localhost:42102"
    assert suite.shots_app_host("https://smartcity.example", "bike") == "https://bike.apps.smartcity.example"


def test_a_role_gated_app_is_judged_after_the_person_signs_in(tmp_path):
    """jc's App host answers a role-gated App with the realm's sign-in form (the App client's own
    code flow), and that form reads no /api/: before the login step every role-gated App failed
    app.page (gate 2026-11-10T203500Z-gc, 22 rows). The shot signs in as the holder and judges the
    page behind the form; without `login` the same page still fails."""
    import http.server
    import threading

    import shots

    class App(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, body=b"", headers=()):
            self.send_response(code)
            for k, v in headers:
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            signed = "sid=holder" in (self.headers.get("Cookie") or "")
            if self.path.endswith("/api/endpoint/x/ngsi-ld/v1/entities"):
                return self._send(200 if signed else 401, b"[]", [("Content-Type", "application/json")])
            page = (b"<title>app</title><script>fetch('/api/endpoint/x/ngsi-ld/v1/entities')</script>" if signed else
                    b"<title>sign in</title><form method=post action=login><input name=username>"
                    b"<input type=password name=password><button>Sign in</button></form>")
            self._send(200, page, [("Content-Type", "text/html")])

        def do_POST(self):
            form = self.rfile.read(int(self.headers["Content-Length"])).decode()
            ok = form == "username=holder&password=pw"
            self._send(302, headers=[("Location", "/"), *([("Set-Cookie", "sid=holder; Path=/")] if ok else [])])

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), App)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    browser = shots.Browser(f"http://127.0.0.1:{srv.server_port}", {}, timeout_ms=5000)
    try:
        rows = {"in": browser.app("gated", tmp_path, "tok", ("holder", "pw")),
                "out": browser.app("gated", tmp_path, "tok")}
    finally:
        browser.close()
        srv.shutdown()
    for r in rows["in"]:
        assert (r.get("signedIn"), r.get("title"), r.get("dataCount"), r.get("bad")) == ("holder", "app", 1, []), r
    for r in rows["out"]:
        assert (r.get("signedIn"), r.get("title"), r.get("dataCount")) == (None, "sign in", 0), r
