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


class Clock:
    """time.time and time.sleep of the suite on a fake clock."""

    def __init__(self, monkeypatch):
        import suite
        self.now = 1_000_000.0
        monkeypatch.setattr(suite.time, "time", lambda: self.now)
        monkeypatch.setattr(suite.time, "sleep", lambda s: setattr(self, "now", self.now + s))


def queue_portal(clock, ahead: int, starts_at: float | None, drain: float | None = None):
    """A Portal whose run-now is Queued behind `ahead` runs of 50 s and starts at `starts_at`
    (never when None); the run then lasts 10 s. With `drain`, one run ahead leaves every `drain`
    seconds and the run-now starts when none is left."""
    import suite
    t0 = clock.now
    if drain is not None:
        starts_at = t0 + ahead * drain

    class P:
        polls = 0

        def call(self, method, path, *a, **k):
            started = starts_at is not None and clock.now >= starts_at
            if method == "POST":
                return 202, {}, b'{"run":"2026-11-09T15:59:15Z"}'
            if path.endswith("/runs?limit=5"):
                P.polls += 1
                if not started:
                    return 200, {}, b'{"items":[]}'
                run = {"run": "2026-11-09T15:59:15Z", "sent": 0, "failed": 0,
                       "lastAt": suite.time.strftime("%Y-%m-%dT%H:%M:%SZ", suite.time.gmtime(starts_at + 10))}
                return 200, {}, json.dumps({"items": [run]}).encode()
            if path.endswith("/log?limit=50"):
                return 200, {}, b'{"items":[{"outcome":"failed","message":"x"}]}'
            if "/pipelines?" in path:
                items = [{"status": {"conditions": [{"message": f"queued, due 2026-11-09T14:49:{i % 60:02d}Z; "
                                                                "the last run ended 2026-11-09T14:40:00Z after 50 s"}]}}
                         for i in range(ahead if drain is None
                                        else max(0, ahead - int((clock.now - t0) // drain)))]
                return 200, {}, json.dumps({"items": items}).encode()
            msg = "running since x" if started else "queued, due 2026-11-09T15:59:15Z"
            return 200, {}, json.dumps({"status": {"conditions": [{"reason": "Queued", "message": msg}]}}).encode()
    return P()


def history(monkeypatch, ahead, starts_after, drain=None):
    import suite
    clock = Clock(monkeypatch)
    portal = queue_portal(clock, ahead, None if starts_after is None else clock.now + starts_after, drain)
    r = suite.Results("pipelines", "http://x")
    p = manifests.Pipeline("banskabystrica", "kataster", "p.yaml", {"period": "168h"}, True)
    t0 = clock.now
    suite.run_pipeline(r, portal, portal, p, 300, ["banskabystrica"])
    return next(c for c in r.rows if c["id"] == "history"), clock.now - t0


def test_a_run_queued_behind_the_queue_is_waited_for_as_long_as_the_runs_ahead_take(monkeypatch):
    """Gate 2026-11-09T155017Z: a run-now behind a queue of runs due earlier (prod's queue order)
    started after the suite's 300 s. Eight runs of 50 s ahead are waited for (400 s); the wait
    never drops below --wait nor passes prod's RUN_TIMEOUT, and a run that never starts is red."""
    row, took = history(monkeypatch, 8, 380)
    assert row["result"] == "pass", row
    row, took = history(monkeypatch, 8, None)
    assert row["result"] == "fail" and "none within 400 s (8 still queued ahead after 1 round" in row["actual"], row
    assert 400 <= took < 420
    row, took = history(monkeypatch, 2, None)
    assert row["result"] == "fail" and "none within 300 s" in row["actual"] and took < 320
    row, took = history(monkeypatch, 500, None)
    assert row["result"] == "fail" and "none within 900 s (500 still queued" in row["actual"] and took < 920


def test_a_run_behind_a_queue_that_moves_is_waited_for_until_it_starts(monkeypatch):
    """Rehearsal 2026-11-10T0255Z: banskabystrica/kataster sat behind 87 queued runs; the statuses
    said 5-50 s a run, the queue drained one every ~28 s, so one estimate (900 s at most) gave up
    while the queue still moved. Prod queues a run-now in due order with no spacing
    (dd7d5ce5 run_queue.rs:767-776), so the suite waits while fewer runs are ahead at each look
    and stops at the first look that shows no progress."""
    row, took = history(monkeypatch, 87, None, drain=28)
    assert row["result"] == "pass", row
    assert 87 * 28 <= took < 87 * 28 + 1300, took


def test_expectations_from_prods_run_log(tmp_path):
    """praha/golemio-garages never ran on prod (run log 2026-10-18..11-09): its refusal is a
    same-on-prod skip; helsinki/vision-counts last ran on prod 2026-10-28 with 1024 sent and holds
    no stream after a run: the held row compares the Endpoint's count with that run."""
    import suite
    (tmp_path / "pipeline_runs.jsonl").write_text("\n".join(json.dumps(r) for r in [
        {"project": "helsinki", "pipeline": "vision-counts", "run": "a", "last_at": "2026-10-27T10:00Z", "sent": 9},
        {"project": "helsinki", "pipeline": "vision-counts", "run": "2026-10-28T10:24:23Z",
         "last_at": "2026-10-28T10:24:52Z", "sent": 1024},
    ]) + "\n")
    prod = suite.prod_runs(tmp_path)
    assert prod["helsinki/vision-counts"]["runs"] == 2 and prod["helsinki/vision-counts"]["last"]["sent"] == 1024
    assert suite.prod_runs(tmp_path / "none") is None

    class H:
        count = "1024"

        def call(self, method, path, *a, **k):
            if path.endswith("/bento"):
                return 404, {}, b""
            if path.startswith("/streams/"):
                return 404, {}, b"Stream not found"
            if "count=true" in path:
                return 200, {"ngsild-results-count": self.count}, b"[]"
            return 200, {}, json.dumps({"metadata": {"name": path.rsplit("/", 1)[1]}, "status": {
                "phase": "Error", "conditions": [{"type": "StreamDeployed", "status": "False", "reason": "RunnerRefused",
                                                  "message": "variable DS_X is not set"}]}}).encode()

    def rows(project, name, prod, h=None):
        r = suite.Results("pipelines", "http://x")
        p = manifests.Pipeline(project, name, "p.yaml", {}, False, source_type="http", slug="s" * 26, out_type="T")
        suite.check_pipeline(r, h or H(), "tok", p, prod)
        return {c["id"]: c for c in r.rows}

    garages = rows("praha", "golemio-garages", prod)["status"]
    assert garages["result"] == "skip" and garages["reason"].startswith("same on prod: prod's Portal run log holds no run")
    # A refusal of a pipeline prod ran stays a fail, and so does any without the run log.
    assert rows("helsinki", "vision-counts", prod)["status"]["result"] == "fail"
    assert rows("praha", "golemio-garages", None)["status"]["result"] == "fail"
    held = rows("helsinki", "vision-counts", prod)["held"]
    assert held["result"] == "pass" and "sent 1024" in held["expected"]
    short = H()
    short.count = "1000"
    assert rows("helsinki", "vision-counts", prod, short)["held"]["result"] == "fail"
    # Without prod's run the held row still wants the stream.
    assert rows("praha", "golemio-garages", prod)["held"]["result"] == "fail"
