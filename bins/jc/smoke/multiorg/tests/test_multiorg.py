"""multiorg's own checks: the realm it makes persons in, and its skip on a one-installation export."""
import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent))
sys.path.insert(0, str(HERE.parent))
import suite  # noqa: E402


def args(tmp_path, state=None):
    exp = tmp_path / "export"
    exp.mkdir()
    (exp / "manifest.json").write_text(json.dumps({"installation": "prod"}))
    return argparse.Namespace(base_url="http://127.0.0.1:9", state=state, export=str(exp))


def test_an_export_without_a_multiorg_block_is_one_named_skip(tmp_path):
    s = suite.Suite(args(tmp_path))
    s.run()
    assert [(r["id"], r["result"]) for r in s.rows] == [("setup", "skip")]
    assert s.rows[0]["reason"].startswith("not applicable: the export is one installation")


def test_the_suite_takes_the_state_realm(tmp_path):
    state = tmp_path / "state"
    state.mkdir()
    (state / "realm-name.txt").write_text("smartcity")
    assert suite.Suite(args(tmp_path, str(state))).realm == "smartcity"
