"""The `rendered` fixture reads the render job's renders when ci hands them over (T-6111).

Each test shard rendered the same three to six environments the render job had already
rendered for the same commit; the shards now fetch that job's files into JC_RENDERED_DIR.
A file there is used as is, and an environment missing from it is still rendered here, so a
missed download only costs time.
"""

import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent

PROBE = """\
def test_probe(rendered):
    print("KINDS", sorted({d.get("kind") for d in rendered("local")}))
"""


def _session(tmp_path, shared: Path | None):
    (tmp_path / "test_probe.py").write_text(PROBE)
    env = {**os.environ, "PYTHONPATH": str(ROOT / "tests")}
    env.pop("JC_RENDERED_DIR", None)
    if shared is not None:
        env["JC_RENDERED_DIR"] = str(shared)
    return subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-s", "-p", "conftest", "-p", "no:cacheprovider",
         "--basetemp", str(tmp_path / "base"), str(tmp_path / "test_probe.py")],
        cwd=tmp_path, env=env, capture_output=True, text=True, timeout=600,
    )


@pytest.fixture
def assembled():
    if not (ROOT / "deployment/environments/local").is_dir():
        pytest.skip("run `just _dev-assemble` first")


def test_a_handed_over_render_is_read_instead_of_rendered(tmp_path, assembled):
    shared = tmp_path / "shared"
    shared.mkdir()
    # No render of an environment has this kind: seeing it proves the file was read.
    (shared / "local.yaml").write_text("kind: HandedOver\nmetadata: {name: probe}\n")
    result = _session(tmp_path, shared)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "KINDS ['HandedOver']" in result.stdout


def test_an_environment_the_job_did_not_hand_over_is_rendered_here(tmp_path, assembled):
    shared = tmp_path / "shared"
    shared.mkdir()
    (shared / "production.yaml").write_text("kind: HandedOver\n")
    result = _session(tmp_path, shared)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "'Deployment'" in result.stdout and "HandedOver" not in result.stdout
