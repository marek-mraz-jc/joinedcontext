"""T-5106: scripts/rotation-drill.py rotates one credential of each kind with probes watching.

rotate-secret.sh itself is held to Runbook 5 by test_rotate_secret.py; here a stand-in records
how the drill calls it, and a local HTTP server plays the probed surfaces.
"""

import importlib.util
import json
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = PROJECT_ROOT / "scripts" / "rotation-drill.py"
spec = importlib.util.spec_from_file_location("rotation_drill", SCRIPT)
drill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drill)

STAND_IN = """#!/usr/bin/env bash
echo "$*" >>"{log}"
case "$*" in *{fail}*) echo "rotate-secret: FAIL: the old value is still accepted"; exit 1 ;; esac
sleep 0.3
echo "rotate-secret: the new value is accepted; rotated"
"""


@pytest.fixture
def stand_in(tmp_path, monkeypatch):
    def make(fail: str = "never-matches") -> Path:
        log = tmp_path / "calls.log"
        script = tmp_path / "rotate-secret.sh"
        script.write_text(STAND_IN.format(log=log, fail=fail))
        script.chmod(0o755)
        monkeypatch.setattr(drill, "ROTATE", script)
        monkeypatch.setattr(drill, "INTERVAL_S", 0.05)
        return log
    return make


@pytest.fixture
def surfaces():
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(500 if self.path == "/down" else 200)
            self.end_headers()

        def log_message(self, *_args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_port}"
    server.shutdown()


def args(tmp_path, *extra):
    return ["--instance", "dev", "--domain", "dev.example", "--realm", "dev", "--out", str(tmp_path / "s.json"), *extra]


def test_a_dry_run_asks_every_kind_for_its_plan_and_reads_no_probe(tmp_path, stand_in):
    log = stand_in()
    assert drill.main(args(tmp_path, "--dry-run")) == 0
    calls = log.read_text().splitlines()
    assert [c.split("--secret ")[1].split()[0] for c in calls] == [
        "keycloak-client-activity-ingest", "db-portal", "gitea-token-gateway", "ckan-api-token"]
    assert all(c.endswith("--dry-run") for c in calls)
    assert "--catalogue https://opendata.dev.example" in calls[3]
    summary = json.loads((tmp_path / "s.json").read_text())
    verdicts = {r["key"]: r["verdict"] for r in summary["results"]}
    assert summary["check"] == "secret-rotation"
    assert verdicts["probes"] == "skip" and verdicts["rotate/agent-runner-model-key"] == "skip"


def test_the_model_key_is_rotated_only_from_a_file_the_owner_supplied(tmp_path, stand_in):
    log = stand_in()
    key = tmp_path / "key"
    key.write_text("x")
    assert drill.main(args(tmp_path, "--dry-run", "--model-key-file", str(key))) == 0
    last = log.read_text().splitlines()[-1]
    assert "--secret agent-runner-model-key" in last and f"--from-file {key}" in last


def test_probes_that_stay_green_pass_the_drill(tmp_path, stand_in, surfaces):
    stand_in()
    out = drill.drill(drill.argparse.Namespace(
        instance="dev", domain="d", realm="dev", model_key_file=None, model_base="", dry_run=False,
        probe=[f"{surfaces}/ok"]))
    probes = [r for r in out if r["key"].startswith("probes/")]
    assert len(probes) == 1 and probes[0]["verdict"] == "pass", probes
    assert "0 of" in probes[0]["title"]


def test_a_probe_that_fails_during_a_rotation_fails_the_drill(tmp_path, stand_in, surfaces):
    stand_in()
    out = drill.drill(drill.argparse.Namespace(
        instance="dev", domain="d", realm="dev", model_key_file=None, model_base="", dry_run=False,
        probe=[f"{surfaces}/ok", f"{surfaces}/down"]))
    verdicts = {r["key"]: r["verdict"] for r in out}
    assert verdicts[f"probes/{surfaces}/ok"] == "pass"
    assert verdicts[f"probes/{surfaces}/down"] == "fail"
    assert " 500" in next(r for r in out if r["key"] == f"probes/{surfaces}/down")["detail"]


def test_a_failed_rotation_fails_the_drill_and_the_rest_still_run(tmp_path, stand_in):
    log = stand_in(fail="db-portal")
    assert drill.main(args(tmp_path, "--dry-run")) == 1
    assert len(log.read_text().splitlines()) == 4
    results = json.loads((tmp_path / "s.json").read_text())["results"]
    failed = [r for r in results if r["verdict"] == "fail"]
    assert [r["key"] for r in failed] == ["rotate/db-portal"]
    assert "still accepted" in failed[0]["title"]


def test_only_https_probes_are_accepted(tmp_path, stand_in):
    stand_in()
    with pytest.raises(SystemExit):
        drill.main(args(tmp_path, "--probe", "http://plain.example/"))


def test_the_summary_is_one_the_health_tab_publishes(tmp_path, stand_in):
    stand_in()
    drill.main(args(tmp_path, "--dry-run"))
    done = subprocess.run([sys.executable, str(PROJECT_ROOT / "scripts" / "publish-health.py"),
                           str(tmp_path / "s.json"), "--every-hours", "744", "--dry-run", "--tasks-dir", str(tmp_path)],
                          capture_output=True, text=True, check=False)
    assert done.returncode == 0, done.stderr
    assert '"check": "secret-rotation"' in done.stdout or '"check":"secret-rotation"' in done.stdout
