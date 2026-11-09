"""Selftests of the image suite on fixture output of image-smoke.sh (the live proof is a run on a
built image).

    python3 -m pytest bins/jc/smoke/image/tests -q
"""

import importlib.util
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
IMAGE = HERE.parent
sys.path.insert(0, str(IMAGE.parent))

from common import Results

_spec = importlib.util.spec_from_file_location("smoke_image_suite", IMAGE / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

PLAN = suite.stages(suite.SCRIPT.read_text())
PATHS = [s[0] for s in PLAN if s[0].startswith("module/")]
OK_LINES = "\n".join(f"ok {i.removeprefix('module')}" for i in PATHS)


def results(text):
    res = Results("image", "http://localhost:1")
    suite.rows_from_output(res, text, PLAN, "sh image-smoke.sh x")
    return {r["id"]: r for r in res.rows}


def test_the_plan_holds_every_check_of_the_script_in_order():
    ids = [s[0] for s in PLAN]
    assert ids[:4] == ["admin.healthz", "user.nonroot", "admin.metrics.anonymous", "admin.metrics.token"]
    assert ids[-2:] == ["gateway.checkout", "gateway.log"]
    # one row per module route of the script: identity, catalogue, dashboards, streams, broker, git, Portal
    assert len(PATHS) == 7 and "module/api/v1/health" in PATHS and "module/streams/ping" in PATHS


def test_a_clean_run_passes_every_row():
    rows = results(f"{OK_LINES}\nok gateway checkout: a b\nSMOKE: ok\n")
    assert len(rows) == len(PLAN) and all(r["result"] == "pass" for r in rows.values())


def test_a_stop_fails_its_row_passes_the_earlier_ones_and_skips_the_rest():
    rows = results("SMOKE: the image does not run as nonroot\n<docker logs>\n")
    assert rows["admin.healthz"]["result"] == "pass"
    assert rows["user.nonroot"]["result"] == "fail"
    assert rows["admin.metrics.token"]["result"] == "skip" and "not reached" in rows["admin.metrics.token"]["reason"]
    mod = PATHS[2]
    rows = results(f"ok /a\nSMOKE: {mod.removeprefix('module')} did not answer with x: <html>\n")
    assert rows[mod]["result"] == "fail" and rows[PATHS[1]]["result"] == "pass"
    assert rows["gateway.checkout"]["result"] == "skip"


def test_a_failure_line_the_plan_does_not_know_and_a_run_without_verdict_fail():
    rows = results("SMOKE: something new broke\n")
    assert rows["script"]["result"] == "fail"
    rows = results("ok /id/realms/jc/.well-known/openid-configuration\n")  # killed mid-run
    assert all(r["result"] == "fail" for r in rows.values())


def test_without_an_image_every_row_is_a_named_skip(tmp_path, monkeypatch):
    monkeypatch.delenv("JC_IMAGE", raising=False)
    assert suite.main(["--base-url", "http://localhost:1", "--out", str(tmp_path)]) == 0
    doc = json.loads((tmp_path / "results.json").read_text())
    assert doc["totals"]["skip"] == len(PLAN) + 2 and doc["totals"]["pass"] == 0
    assert all("--image" in r["reason"] for r in doc["checks"])
