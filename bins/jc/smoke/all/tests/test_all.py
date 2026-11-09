import pytest
import json
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import suite as all_suite

GOOD = '''
import argparse, sys
from pathlib import Path
from common import Results
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out"); p.add_argument("--export", required=True)
    a = p.parse_args(argv)
    r = Results("good", a.base_url)
    r.check("one", "t", "GET /", "200", "200", True)
    r.check("two", "t", "GET /x", "200", "200", a.export == "E")
    r.skip("three", "t", "GET /y", "200", "no data")
    r.write(Path(a.out))
    return 0 if r.totals()["fail"] == 0 else 1
'''
DIES = '''
def main(argv):
    raise RuntimeError("boom")
'''
LOGS = '''
import argparse, json
from pathlib import Path
from common import Results
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out"); p.add_argument("--log-file")
    p.add_argument("--results-dir", action="append", default=[])
    a = p.parse_args(argv)
    r = Results("logs", a.base_url)
    r.check("dirs", "t", "scan", "2 dirs", str(len(a.results_dir)), len(a.results_dir) == 2)
    r.check("log", "t", "read", "L", str(a.log_file), a.log_file == "L")
    r.write(Path(a.out))
    return 0
'''


def smoke(tmp: Path, suites: dict) -> Path:
    root = tmp / "smoke"
    root.mkdir()
    for f in ("run", "common.py"):
        shutil.copy(all_suite.SMOKE / f, root / f)
    (root / "all").mkdir()
    for name, text in suites.items():
        (root / name).mkdir()
        (root / name / "suite.py").write_text(text)
    return root


def test_every_suite_runs_with_only_the_flags_it_declares_and_logs_goes_last(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path, {"good": GOOD, "dies": DIES, "logs": LOGS}))
    out = tmp_path / "out"
    code = all_suite.main(["--base-url", "http://127.0.0.1:1", "--out", str(out), "--export", "E",
                           "--log-file", "L"])
    doc = json.loads((out / "results.json").read_text())
    by = {s["suite"]: s for s in doc["suites"]}
    assert [s["suite"] for s in doc["suites"]] == ["dies", "good", "logs", "e2e"]
    assert (by["good"]["pass"], by["good"]["fail"], by["good"]["skip"]) == (2, 0, 1)
    assert by["dies"]["fail"] == 1 and "boom" in doc["checks"][0]["reason"]
    assert by["logs"]["pass"] == 2  # handed --log-file and both earlier result dirs, not --export
    assert by["e2e"]["skip"] == 1
    assert doc["verdict"] == "RED" and code == 1
    assert "| good | 3 | 2 | 0 | 0 | 1 |" in (out / "REPORT.md").read_text()
    ids = {r["id"] for r in doc["checks"]}
    assert {"good/one", "good/two", "good/three", "dies/run"} <= ids


def test_green_when_nothing_fails_and_a_missing_required_flag_is_a_skip(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path, {"good": GOOD}))
    out = tmp_path / "out"
    assert all_suite.main(["--base-url", "http://localhost:1", "--out", str(out), "--export", "E"]) == 0
    assert json.loads((out / "results.json").read_text())["verdict"] == "GREEN"
    out2 = tmp_path / "out2"
    # No --export: the suite refuses to start; that is a skip, and a run with no pass is RED.
    assert all_suite.main(["--base-url", "http://localhost:1", "--out", str(out2)]) == 1
    doc = json.loads((out2 / "results.json").read_text())
    assert doc["checks"][0]["result"] == "skip" and "--export" in doc["checks"][0]["reason"]
    assert doc["verdict"] == "RED"


def test_a_suite_past_its_timeout_fails(tmp_path, monkeypatch):
    slow = "import time\ndef main(argv):\n    time.sleep(30)\n"
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path, {"slow": slow}))
    out = tmp_path / "out"
    assert all_suite.main(["--base-url", "http://localhost:1", "--out", str(out), "--suite-timeout", "1"]) == 1
    row = json.loads((out / "results.json").read_text())["checks"][0]
    assert row["result"] == "fail" and "timeout" in row["actual"]


def test_refuses_a_host_that_is_not_loopback(tmp_path):
    try:
        all_suite.main(["--base-url", "https://api.dev.joinedcontext.com", "--out", str(tmp_path)])
    except SystemExit as e:
        assert "refusing" in str(e)
    else:
        raise AssertionError("ran against a remote host")


def test_cargo_test_output_becomes_one_row_per_test():
    text = """   Compiling jc v0.1.0
     Running unittests src/main.rs (target/debug/deps/jc-0123)
test modules::gateway::tests::reads ... ok
     Running tests/one_binary.rs (target/debug/deps/one_binary-4567)
test every_module_answers ... FAILED
test slow_one ... ignored
"""
    rows = all_suite.parse_e2e(text, "cargo test")
    assert [(r["id"], r["result"]) for r in rows] == [
        ("e2e/main::modules::gateway::tests::reads", "pass"),
        ("e2e/one_binary::every_module_answers", "fail"),
        ("e2e/one_binary::slow_one", "skip"),
    ]


def test_a_known_gap_stays_listed_but_only_a_new_failure_turns_the_verdict_red(tmp_path, monkeypatch):
    bad = GOOD.replace('a.export == "E"', "False")
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path, {"good": bad}))
    known = tmp_path / "known.txt"
    known.write_text("# comment line\n^good/two$  # filed: T-1, owner x\n^good/gone$  # fixed since\n")
    out = tmp_path / "out"
    code = all_suite.main(["--base-url", "http://localhost:1", "--out", str(out), "--export", "E",
                           "--known", str(known)])
    doc = json.loads((out / "results.json").read_text())
    assert code == 0 and doc["verdict"] == "GREEN" and doc["newFailures"] == 0
    two = next(r for r in doc["checks"] if r["id"] == "good/two")
    assert two["result"] == "fail" and two["known"] == "filed: T-1, owner x"
    assert doc["knownFixed"] == ["^good/gone$"]
    report = (out / "REPORT.md").read_text()
    assert "| good | 3 | 1 | 1 | 1 | 1 |" in report and "good/two" in report
    known.write_text("")
    assert all_suite.main(["--base-url", "http://localhost:1", "--out", str(out), "--export", "E",
                           "--known", str(known)]) == 1


def test_the_realm_is_the_state_files_name_else_jc_and_a_bad_name_stops(tmp_path):
    from common import realm_of
    assert realm_of(None) == "jc"
    assert realm_of(tmp_path) == "jc"
    (tmp_path / "realm-name.txt").write_text("smartcity\n")
    assert realm_of(tmp_path) == "smartcity"
    assert realm_of(str(tmp_path)) == "smartcity"
    for bad in ("master", "Master", "../x", "a/b", "a b", ".x"):
        (tmp_path / "realm-name.txt").write_text(bad)
        with pytest.raises(SystemExit):
            realm_of(tmp_path)
