"""T-5564: every action a workflow runs is a commit, and only the job that publishes may publish.

scripts/ci/check-workflow-pins.py is the gate (ci.yml runs it first); these tests keep it honest:
it passes on this repository and goes red on each defect it exists for.
"""

import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/ci/check-workflow-pins.py"


def run(*args):
    return subprocess.run([sys.executable, str(SCRIPT), *args], check=False, capture_output=True, text=True, timeout=60)


def test_every_workflow_is_pinned_and_scoped():
    result = run()
    assert result.returncode == 0, result.stderr


def test_the_check_is_red_on_a_planted_tag_and_a_workflow_wide_grant():
    result = run("--selftest")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "5 planted defects caught" in result.stdout
