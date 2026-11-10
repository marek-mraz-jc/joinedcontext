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
    assert not suite.server_judge(0, {}, b"connection refused", False)[1]
