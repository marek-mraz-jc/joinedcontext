import pytest
"""Unit tests of the pipelines suite and bench (no jc needed):
python3 -m pytest bins/jc/smoke/pipelines/tests"""

import json
import sys
import textwrap
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent), str(HERE.parents[1])]

import bench  # noqa: E402
import manifests  # noqa: E402
from proxy import entities_in  # noqa: E402


def write(root: Path, rel: str, text: str):
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(textwrap.dedent(text))


def test_pipelines_carry_their_source_type_target_slug_and_type(tmp_path):
    write(tmp_path, "o/city/spaces/verejne/endpoints/e.yaml", """
        kind: Endpoint
        metadata: {name: city-verejne, namespace: city}
        spec: {contextSpaceRef: verejne, slug: aaaabbbbccccddddeeeeffffgggghhhh}
    """)
    write(tmp_path, "o/city/datasources/feed.yaml", """
        kind: DataSource
        metadata: {name: feed, namespace: city}
        spec: {type: mqtt}
    """)
    write(tmp_path, "o/city/pipelines/a/pipeline.yaml", """
        kind: Pipeline
        metadata: {name: a, namespace: city}
        spec:
          schedule: '0 3 * * *'
          source: {dataSourceRef: {kind: DataSource, name: feed}}
          targetEndpoint: urn:ngsi-ld:Endpoint:city.example:city-verejne:city-verejne
          output: {type: Streetlight, mode: upsert}
          secretRefs: [{name: key, key: token, envVar: CITY_KEY}]
    """)
    write(tmp_path, "o/city/pipelines/a/bento.yaml", "pipeline: {processors: []}\n")
    write(tmp_path, "o/city/pipelines/b/pipeline.yaml", """
        kind: Pipeline
        metadata: {name: b, namespace: city}
        spec:
          sources: [{dataSourceRef: feed}]
          outputs: [{targetEndpoint: 'urn:ngsi-ld:Endpoint:city-verejne', type: Lamp}]
    """)
    a, b = manifests.load_pipelines(tmp_path)
    assert (a.key, a.source_type, a.out_type, a.bento, a.secret_refs) == \
        ("city/a", "mqtt", "Streetlight", True, ["key"])
    assert a.slug == b.slug == "aaaabbbbccccddddeeeeffffgggghhhh"
    assert a.resident and a.scheduled
    assert (b.out_type, b.bento, b.secret_refs) == ("Lamp", False, [])


def test_a_batch_body_counts_its_entities_or_its_queried_ids():
    assert entities_in("entityOperations/upsert", json.dumps([{"id": 1}, {"id": 2}]).encode()) == 2
    q = {"type": "Query", "entities": [{"type": "*", "id": ["a", "b", "c"]}]}
    assert entities_in("entityOperations/query", json.dumps(q).encode()) == 3
    assert entities_in("entities", b"[]") is None


def rendered():
    """The shape the Portal renders (tests/portal-contract streams.jsonl.gz), cut down."""
    http = {"url": "${JC_GATEWAY_URL}/api/endpoint/slug/ngsi-ld/v1/entityOperations/upsert?options=update",
            "verb": "POST", "drop_on": [400, 413, 422], "timeout": "30s", "rate_limit": "p__pipeline_egress",
            "oauth2": {"enabled": True, "client_key": "p/n", "token_url": "${JC_PIPELINE_TOKEN_URL}"}}
    return {"input": {"generate": {"count": 1, "interval": "", "mapping": 'root = ""'}},
            "pipeline": {"processors": [
                {"label": "tap_input", "branch": {}},
                {"label": "processor_0", "mapping": "root = this"},
                {"label": "processor_1", "mapping": bench.BATCH_MAPPING},
                {"label": "processor_2", "unarchive": {"format": "json_array"}},
                {"label": "validation", "switch": []},
                {"label": "processor_3", "split": {"size": 1000, "byte_size": 1048576}},
                {"label": "processor_4", "archive": {"format": "json_array"}},
                {"label": "write_0", "branch": {"processors": [{"http": http}], "result_map": "x"}},
                {"label": "write_refused", "switch": []},
                {"label": "tap_output", "branch": {}},
            ]},
            "output": {"drop": {}}}


def test_native_replaces_split_archive_and_the_upsert_branch_with_one_ngsi_ld_writer():
    s = bench.native(rendered(), 4)
    labels = [p.get("label") for p in s["pipeline"]["processors"]]
    assert labels == ["tap_input", "processor_0", "processor_1", "processor_2", "validation", "write_0", "tap_output"]
    w = s["pipeline"]["processors"][5]["ngsi_ld"]
    assert w["url"] == "${JC_GATEWAY_URL}/api/endpoint/slug/ngsi-ld/v1"
    assert (w["options"], w["max_in_flight"], w["rate_limit"]) == ("update", 4, "p__pipeline_egress")
    assert w["oauth2"]["client_key"] == "p/n"


def test_synthetic_keeps_the_write_path_and_drops_source_steps_and_validation():
    s = bench.synthetic(rendered(), 5000)
    labels = [p.get("label") for p in s["pipeline"]["processors"]]
    assert labels == ["tap_input", "processor_1", "processor_2", "processor_3", "processor_4", "write_0",
                      "write_refused", "tap_output"]
    assert "range(0, 5000)" in s["input"]["generate"]["mapping"]
    twice = bench.synthetic(rendered(), 10, half=5)["input"]["generate"]["mapping"]
    assert "(i % 5)" in twice and '"seq"' in twice


def test_the_sweep_stream_queries_and_deletes_in_chunks_of_the_given_size():
    ids = [f"urn:ngsi-ld:T:{i}" for i in range(250)]
    s = bench.source_deletes("p", "n", "slug", "r", ids, 100, "http://portal-internal.test/x")
    url = s["pipeline"]["processors"][1]["try"][0]["branch"]["processors"][0]["http"]["url"]
    assert url.endswith("entityOperations/query?options=sysAttrs,keyValues&limit=100")
    import base64
    enc = s["input"]["generate"]["mapping"].split('"')[1]
    chunks = json.loads(base64.b64decode(enc))
    assert [len(c["ids"]) for c in chunks] == [100, 100, 50]


def test_the_runner_check_refuses_the_portal_page_answered_in_place_of_ekiben():
    import suite
    assert suite.runner_list({"content-type": "text/html"}, b"<!doctype html>") is None
    assert suite.runner_list({"content-type": "application/json"}, b"not json") is None
    assert suite.runner_list({"content-type": "application/json"}, b'{"zilina.a": {}}') == {"zilina.a": {}}


def test_the_runner_check_sends_the_ekiben_token_jc_was_given_as_a_file(tmp_path):
    """jc removes <state>/ekiben-token when given --ekiben-token-file: the suite reads
    <home>/secrets/ekiben-token instead, or ekiben answers its stream list 401."""
    import threading
    from http.server import BaseHTTPRequestHandler, HTTPServer

    import suite
    from common import Http

    class Ekiben(BaseHTTPRequestHandler):
        def do_GET(self):
            ok = self.headers.get("Authorization") == "Bearer t0ken"
            self.send_response(200 if ok else 401)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b"{}" if ok else b'{"error":"unauthorized"}')

        def log_message(self, *a):
            pass

    (tmp_path / "state").mkdir()
    (tmp_path / "secrets").mkdir()
    (tmp_path / "secrets" / "ekiben-token").write_text("t0ken\n")
    srv = HTTPServer(("127.0.0.1", 0), Ekiben)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    try:
        http = Http(f"http://127.0.0.1:{srv.server_port}")
        assert http.call("GET", "/streams/streams", None)[0] == 401
        tok = suite.ekiben_token(tmp_path / "state")
        assert http.call("GET", "/streams/streams", tok)[0] == 200
    finally:
        srv.shutdown()
    assert suite.ekiben_token(tmp_path / "nothing" / "state") is None


def test_the_admin_signs_in_at_the_realm_the_state_names(tmp_path, monkeypatch):
    import common
    import people
    seen = []
    monkeypatch.setattr(common.Http, "call", lambda self, m, path, *a, **k: seen.append(path) or (401, {}, b""))
    (tmp_path / "portal-admin-client-secret").write_text("x")
    (tmp_path / "realm-name.txt").write_text("smartcity")
    with pytest.raises(RuntimeError, match="401"):
        people.Admin("http://127.0.0.1:9", tmp_path)
    assert seen == ["/id/realms/smartcity/protocol/openid-connect/token"]


def test_a_schedule_prods_portal_refuses_too_is_a_named_same_on_prod_skip(monkeypatch):
    import suite
    assert manifests.cron_out_of_range("77 3 * * 3") == "minute"
    assert manifests.cron_out_of_range("0 25 * * *") == "hour"
    for ok in ("0 5 * * *", "*/15 0-23 1,15 * 7", "", "0 5 * *"):
        assert manifests.cron_out_of_range(ok) is None, ok

    class H:
        def call(self, method, path, *a, **k):
            if path.endswith("/bento"):
                return 404, {}, b""
            return 200, {}, json.dumps({"metadata": {"name": "noise-rail"}, "status": {
                "phase": "Error", "conditions": [{"type": "StreamDeployed", "status": "False", "reason": "RunnerRefused",
                                                  "message": "schedule `77 3 * * 3` is not five cron fields"}]}}).encode()

    def rows(schedule):
        r = suite.Results("pipelines", "http://x")
        p = manifests.Pipeline("helsinki", "noise-rail", "p.yaml", {"schedule": schedule}, False)
        suite.check_pipeline(r, H(), None, p)
        return {c["id"]: c for c in r.rows}
    status = rows("77 3 * * 3")["status"]
    assert status["result"] == "skip" and status["reason"].startswith("same on prod: schedule minute out of range")
    # A refusal of a schedule prod would take stays a fail.
    assert rows("7 3 * * 3")["status"]["result"] == "fail"


def test_a_run_once_http_pipeline_holds_no_stream_and_its_held_row_is_a_same_on_prod_skip(tmp_path):
    """helsinki/vision-counts: no period, no schedule, an http source. Prod's Portal queues it as
    Clock::Once and the runner holds its stream only while the one run lasts, so a 404 from the
    runner is prod's behaviour; a resident source's 404 stays a fail."""
    import suite
    write(tmp_path, "o/helsinki/datasources/file.yaml", """
        kind: DataSource
        metadata: {name: file, namespace: helsinki}
        spec: {type: http, http: {url: 'https://example.org/x.json'}}
    """)
    write(tmp_path, "o/helsinki/datasources/feed.yaml", """
        kind: DataSource
        metadata: {name: feed, namespace: helsinki}
        spec: {type: mqtt}
    """)
    for name, ds in (("once", "file"), ("live", "feed")):
        write(tmp_path, f"o/helsinki/pipelines/{name}/pipeline.yaml", f"""
            kind: Pipeline
            metadata: {{name: {name}, namespace: helsinki}}
            spec: {{class: auto, source: {{dataSourceRef: {{kind: DataSource, name: {ds}}}}}}}
        """)
    live, once = manifests.load_pipelines(tmp_path)
    assert once.runs_once and not live.runs_once

    class H:
        def call(self, method, path, *a, **k):
            if path.startswith("/streams/"):
                return 404, {}, b'{"error":"Stream not found"}'
            return 200, {}, json.dumps({"metadata": {"name": path.rsplit("/", 1)[-1]}, "status": {}}).encode()

    def held(p):
        r = suite.Results("pipelines", "http://x")
        suite.check_pipeline(r, H(), "tok", p)
        return next(c for c in r.rows if c["id"] == "held")
    assert held(once)["result"] == "skip" and held(once)["reason"].startswith("same on prod: no period")
    assert held(live)["result"] == "fail"
