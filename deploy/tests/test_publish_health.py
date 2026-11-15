"""T-2803: scripts/publish-health.py turns a check's summary into the digest the Portal shows.

No cluster here: kubectl is replaced, and the digest is checked against the limits the Portal
enforces (API/01 §26), so a digest this script writes is never shown as unreadable.
"""

import importlib.util
import json
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "publish-health.py"
spec = importlib.util.spec_from_file_location("publish_health", SCRIPT)
ph = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ph)

NOW = datetime(2026, 9, 25, 10, 0, tzinfo=timezone.utc)


def summary(*results, check="deployment"):
    return {"check": check, "repo": "joinedcontext-deployment", "run": "dev-validate 1",
            "results": [{"key": k, "verdict": v, "title": t, "detail": "d", "evidence": "e"} for k, v, t in results]}


def test_counts_failures_and_nothing_but_keys_titles_and_verdicts():
    body = ph.digest(summary(("a", "pass", "ok"), ("b", "fail", "broken"), ("c", "error", "no answer"),
                             ("d", "skip", "n/a")), 1, NOW, None, {})
    assert body["counts"] == {"pass": 1, "fail": 1, "error": 1, "skip": 1}
    assert body["failures"] == [{"key": "b", "verdict": "fail", "title": "broken"},
                                {"key": "c", "verdict": "error", "title": "no answer"}]
    assert body["history"] == [{"at": "2026-09-25T10:00:00Z", "pass": 1, "fail": 1, "error": 1, "skip": 1}]
    assert "detail" not in json.dumps(body) and "evidence" not in json.dumps(body)


def test_a_secret_in_a_title_or_the_run_is_redacted_and_long_text_cut():
    token = "Bearer " + "a" * 30
    s = summary(("k", "fail", f"login as {token} password=hunter2 " + "x " * 400))
    s["run"] = "https://ci:s3cret@example.org/run/1"
    body = ph.digest(s, 1, NOW, None, {})
    title = body["failures"][0]["title"]
    assert "hunter2" not in title and "a" * 30 not in title and len(title) <= ph.MAX_TEXT
    assert "s3cret" not in body["run"]


def test_each_failure_names_its_open_task_or_the_checks_overflow_task(tmp_path):
    (tmp_path / "T-2901-x.md").write_text("---\nid: T-2901\n---\ncheck: deployment/images/portal\n")
    (tmp_path / "T-2902-y.md").write_text("---\nid: T-2902\n---\ncheck: deployment/*overflow*\n")
    (tmp_path / "T-2903-z.md").write_text("---\nid: T-2903\n---\ncheck: conformance/images/portal\n")
    (tmp_path / "finished").mkdir()
    (tmp_path / "finished" / "T-2800-old.md").write_text("---\nid: T-2800\n---\ncheck: deployment/drift/x\n")
    tasks = ph.open_tasks(tmp_path, "deployment")
    assert tasks == {"images/portal": "T-2901", "*overflow*": "T-2902"}
    body = ph.digest(summary(("images/portal", "fail", "unsigned"), ("drift/x", "fail", "drifted")), 1, NOW, None, tasks)
    assert [f.get("task") for f in body["failures"]] == ["T-2901", "T-2902"]


def test_the_history_keeps_seven_days_and_at_most_200_points_and_drops_a_broken_one():
    old = {"at": (NOW - timedelta(days=8)).strftime("%Y-%m-%dT%H:%M:%SZ"), "pass": 1, "fail": 0, "error": 0, "skip": 0}
    many = [{"at": (NOW - timedelta(minutes=i + 1)).strftime("%Y-%m-%dT%H:%M:%SZ"), "pass": 1, "fail": 0,
             "error": 0, "skip": 0} for i in range(250)][::-1]
    previous = {"history": [old, {"at": "yesterday"}, {"pass": 1}] + many}
    history = ph.digest(summary(("a", "pass", "ok")), 1, NOW, previous, {})["history"]
    assert len(history) == ph.MAX_HISTORY and history[-1]["at"] == "2026-09-25T10:00:00Z"
    assert old not in history


def test_failures_stop_at_the_portals_limit():
    results = [(f"k{i}", "fail", "t") for i in range(80)]
    body = ph.digest(summary(*results), 1, NOW, None, {})
    assert body["counts"]["fail"] == 80 and len(body["failures"]) == ph.MAX_FAILURES


@pytest.mark.parametrize("bad", [summary(check="Bad Name"), {"check": "x"},
                                 summary(("a", "passed", "t"))])
def test_a_summary_that_is_not_one_is_refused(bad):
    with pytest.raises(SystemExit):
        ph.digest(bad, 1, NOW, None, {})


def test_every_check_applies_only_its_own_key(monkeypatch, tmp_path):
    calls = []

    def run(cmd, input=None, **kw):
        calls.append((cmd, input))
        if cmd[1:3] == ["get", "pods"]:
            return subprocess.CompletedProcess(cmd, 0, "dev", "")
        if cmd[1:3] == ["get", "configmap"]:
            raise subprocess.CalledProcessError(1, cmd, "", 'Error from server (NotFound): configmaps "x" not found')
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(ph.subprocess, "run", run)
    path = tmp_path / "s.json"
    path.write_text(json.dumps(summary(("a", "pass", "ok"))))
    assert ph.main([str(path), "--every-hours", "1", "--tasks-dir", str(tmp_path)]) == 0
    cmd, body = calls[-1]
    assert cmd[:3] == ["kubectl", "apply", "--server-side"] and "--field-manager=publish-health-deployment" in cmd
    applied = json.loads(body)
    assert applied["metadata"] == {"name": "jc-validation-results", "namespace": "dev",
                                   "labels": {"app.kubernetes.io/part-of": "joinedcontext"}}
    assert list(applied["data"]) == ["deployment.json"]


def test_a_dry_run_touches_no_cluster(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(ph.subprocess, "run", lambda *a, **k: pytest.fail("kubectl was called"))
    path = tmp_path / "s.json"
    path.write_text(json.dumps(summary(("a", "fail", "x"))))
    assert ph.main([str(path), "--every-hours", "24", "--dry-run", "--tasks-dir", str(tmp_path)]) == 0
    assert json.loads(capsys.readouterr().out)["everyHours"] == 24


def test_every_hours_is_bounded(tmp_path):
    with pytest.raises(SystemExit):
        ph.main([str(tmp_path / "s.json"), "--every-hours", "0", "--dry-run"])


def test_the_app_probe_lists_its_passing_keys_and_other_checks_do_not():
    s = summary(("helsinki/bikes", "pass", "ok"), ("helsinki/alerts", "fail", "no row"), check="apps")
    assert ph.digest(s, 1, NOW, None, {}, passed=True)["passed"] == ["helsinki/bikes"]
    assert "passed" not in ph.digest(s, 1, NOW, None, {})


def test_more_passing_keys_than_the_portal_shows_is_refused():
    s = summary(*[(f"p/a{i}", "pass", "ok") for i in range(ph.MAX_PASSED + 1)], check="apps")
    with pytest.raises(SystemExit):
        ph.digest(s, 1, NOW, None, {}, passed=True)
