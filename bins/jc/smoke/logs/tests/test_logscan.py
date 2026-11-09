import pytest
import base64
import json
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import logscan as ls  # noqa: E402
import suite  # noqa: E402
from common import Results  # noqa: E402

TEXT = ("2026-11-08T21:36:57.665694Z  INFO request{request_id=8b26b8d43a8ae3ba831147f01495391f}: "
        "joinedcontext_portal::auth::oidc: portal login subject=01a1bd0f-c305-71f9 reason=\"no token here\"")


def test_a_text_line_names_its_module_request_id_and_fields():
    r = ls.parse_line(1, TEXT)
    assert r.structured and r.level == "INFO" and r.module == "portal"
    assert r.request_id == "8b26b8d43a8ae3ba831147f01495391f"
    assert r.message == "portal login" and r.fields["reason"] == "no token here"
    assert r.ts.year == 2026


def test_a_json_line_and_a_loose_line():
    j = json.dumps({"timestamp": "2026-11-08T21:36:57.1234567Z", "level": "WARN", "target": "antares_broker::x",
                    "fields": {"message": "refused", "status": 401},
                    "span": {"name": "request", "request_id": "smokelogs-abc12345"}})
    r = ls.parse_line(1, j)
    assert r.structured and r.module == "antares" and r.request_id == "smokelogs-abc12345"
    assert r.message == "refused" and r.ts is not None
    assert not ls.parse_line(2, "   continuation of something").structured


def test_targets_map_to_modules():
    assert ls.module_of("context_gateway::mcp::hub") == "gateway"
    assert ls.module_of("ekiben_server::api") == "ekiben"
    assert ls.module_of("jc::modules::shako") == "shako"
    assert ls.module_of("jc::app") == "jc"
    assert ls.module_of("sqlx::query") == "other"
    r = ls.parse_line(1, "2026-11-08T21:36:57Z  INFO jc::access: request module=dataroom status=401")
    assert r.module == "dataroom"
    r = ls.parse_line(1, '2026-11-08T21:36:57Z  INFO request{module="gateway" request_id=smokelogs-0123}: '
                         'jc_core::auth::token: token refused rejected="no bearer token presented"')
    assert r.module == "gateway" and r.request_id == "smokelogs-0123"
    r = ls.parse_line(1, '2026-11-08T21:36:57Z  INFO request{module="portal" request_id=abcdefgh1}: '
                         'shako_api::x: a line of shako inside a Portal span')
    assert r.module == "shako"


def test_every_form_of_a_secret_is_found_and_names_only_its_file(tmp_path):
    (tmp_path / "token").write_text("s3cretValue-0123456789abcdef\n")
    (tmp_path / "db-url").write_text("postgres://app:p%40ssw0rd-longenough@127.0.0.1:5432/db\n")
    (tmp_path / "plain-url").write_text("postgres://app@127.0.0.1:5432/db\n")
    (tmp_path / "c.json").write_text(json.dumps({"client_secret": "jsonSecretValue123456", "name": "visible-name-1234"}))
    sub = tmp_path / "checkout"
    sub.mkdir()
    (sub / "HEAD").write_text("0123456789abcdef0123456789abcdef01234567\n")
    found = ls.collect_secrets([tmp_path])
    names = {s.name for s in found}
    assert names == {f"{tmp_path.name}/token", f"{tmp_path.name}/db-url", f"{tmp_path.name}/c.json:client_secret"}
    idx = ls.SecretIndex(found)
    b64 = base64.b64encode(b"s3cretValue-0123456789abcdef").decode()
    assert [s.name for s in idx.find(f"x=1 header={b64} y")] == [f"{tmp_path.name}/token"]
    assert [s.name for s in idx.find("pw=p%40ssw0rd-longenough;")] == [f"{tmp_path.name}/db-url"]
    assert [s.name for s in idx.find("...prefixjsonSecretValue123456suffix")] == [f"{tmp_path.name}/c.json:client_secret"]
    assert idx.find("visible-name-1234 and 0123456789abcdef0123456789abcdef01234567") == []


def test_credential_shapes_and_personal_data():
    part = lambda d: base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip("=")  # noqa: E731
    jwt = ".".join([part({"alg": "none"}), part({"sub": "not-a-person"}), part({"sig": "not-a-signature"})])
    assert ls.credential_shapes(f"token {jwt}") == ["jwt"]
    assert ls.credential_shapes("Authorization: Bearer abcdefghijklmnop1234") == ["bearer"]
    assert ls.credential_shapes('password="hunter2hunter2"') == ["password-field"]
    assert ls.credential_shapes("password=<redacted> secret=$(env:X_SECRET)") == []
    found = ls.personal_data("user anna.k@example.org from 8.8.4.4 via 127.0.0.1 and 10.1.2.3, crate foo@1.2.3")
    assert found == ["email a***@example.org", "ip 8.8.x.x"]
    assert ls.personal_data("noreply@joinedcontext.com subject=01a1bd0f-c305") == []


def test_volume_counts_lines_per_minute_and_the_top_message():
    lines = [f"2026-11-08T21:{m:02d}:00Z  WARN joinedcontext_portal::reconciler::daemon: same warning path=x"
             for m in range(10)]
    lines.append("2026-11-08T21:09:30Z  INFO antares_broker: started")
    v = ls.volume([ls.parse_line(i, x) for i, x in enumerate(lines)])
    assert v["portal"]["lines"] == 10 and v["portal"]["topCount"] == 10
    assert v["portal"]["linesPerMinute"] == round(10 / 9.5, 1)
    assert v["antares"]["lines"] == 1


def records(*lines):
    return [ls.parse_line(i, x) for i, x in enumerate(lines, 1)]


def probe(rid, status=401, echoed=True, t=None):
    t = t or time.time()
    return {"module": "shako", "method": "GET", "path": "/git/api/v1/user", "ok": (401,), "rid": rid,
            "status": status, "headers": {"x-request-id": rid} if echoed else {}, "body": b"", "t0": t, "t1": t}


def by_id(res):
    return {r["id"]: r["result"] for r in res.rows}


def test_a_refusal_is_judged_by_the_line_that_carries_its_request_id():
    rid = "smokelogs-0123456789abcdef"
    res = Results("logs", "http://localhost:1")
    suite.judge_refused(res, probe(rid), records(
        f"2026-11-08T21:00:00Z  INFO request{{request_id={rid}}}: shako_api::auth: refused reason=\"no token\""))
    assert by_id(res) == {"refused.shako.status": "pass", "refused.shako.echo": "pass",
                          "refused.shako.logged": "pass", "refused.shako.reason": "pass"}
    # Two lines for one refusal, no echo: logged and echo fail.
    res = Results("logs", "http://localhost:1")
    line = f"2026-11-08T21:00:00Z  INFO jc::access: refused request_id={rid} module=shako status=401"
    suite.judge_refused(res, probe(rid, echoed=False), records(line, line))
    got = by_id(res)
    assert got["refused.shako.echo"] == "fail" and got["refused.shako.logged"] == "fail"


def test_a_refusal_nobody_logged_fails_and_a_dead_door_skips():
    rid = "smokelogs-fedcba9876543210"
    res = Results("logs", "http://localhost:1")
    suite.judge_refused(res, probe(rid, echoed=False), records(
        "2026-11-08T21:00:00Z  INFO joinedcontext_portal::x: unrelated"))
    assert by_id(res)["refused.shako.reason"] == "fail"
    res = Results("logs", "http://localhost:1")
    suite.judge_refused(res, probe(rid, status=0), [])
    got = by_id(res)
    assert got["refused.shako.status"] == "fail" and got["refused.shako.logged"] == "skip"


def test_a_flow_names_the_modules_that_dropped_the_id():
    rid = "smokelogs-1111222233334444"
    res = Results("logs", "http://localhost:1")
    suite.judge_flow(res, "portal-shako", "GET x", rid, ("portal", "shako"), records(
        f"2026-11-08T21:00:00Z  INFO request{{request_id={rid}}}: joinedcontext_portal::api: read"), 200)
    row = res.rows[0]
    assert row["result"] == "fail" and "shako" in row["reason"] and "portal" not in row["reason"]


def test_the_suite_windows_count_only_passed_writes(tmp_path):
    d = tmp_path / "mcp"
    d.mkdir()
    (d / "results.json").write_text(json.dumps({"suite": "mcp", "seconds": 100, "checks": [
        {"call": "POST /api/v1/connectors", "result": "pass"},
        {"call": "GET /api/v1/projects", "result": "pass"},
        {"call": "tools/call jc_resource_propose {...}", "result": "pass"},
        {"call": "DELETE /api/v1/connectors/x", "result": "fail"}]}))
    os.utime(d / "results.json", (1_000_000, 1_000_000))
    (w,) = suite.suite_windows([d])
    assert w["writes"] == 2 and w["start"] == 1_000_000 - 100 - 60 and w["end"] == 1_000_000 + 60


def test_metric_names_drop_labels_and_comments():
    text = "# HELP x y\nantares_up 1\njc_portal_requests_total{route=\"/\"} 3\n"
    assert suite.metric_names(text) == {"antares_up", "jc_portal_requests_total"}


def test_the_reader_and_the_refused_token_call_use_the_state_realm(tmp_path, monkeypatch):
    seen = []
    monkeypatch.setattr(suite.Session, "call", lambda self, m, path, *a, **k: seen.append(path) or (401, {}, b""))
    (tmp_path / "portal-admin-client-secret").write_text("x")
    (tmp_path / "realm-name.txt").write_text("smartcity")
    with pytest.raises(RuntimeError, match="401"):
        suite.Person("http://127.0.0.1:9", tmp_path)._admin_token()
    assert seen == ["/id/realms/smartcity/protocol/openid-connect/token"]
    kaisatsu = [c for c in suite.refused_calls("m", None, "smartcity") if c["module"] == "kaisatsu"]
    assert [c["path"] for c in kaisatsu] == ["/id/realms/smartcity/protocol/openid-connect/token"]
