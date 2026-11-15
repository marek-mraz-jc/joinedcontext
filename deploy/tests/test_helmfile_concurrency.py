"""T-3095: every helmfile run the justfile starts caps how many helm processes run at once.

Unbounded (helmfile's default 0), the hourly dev-apply started dozens of helm processes beside the
agents' builds and the server reached load 335 on 8 CPUs, with SSH timing out."""

import os
import pathlib
import re
import shutil
import subprocess

import pytest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPAWNS = re.compile(r"\bhelmfile\b[^|#]*?\s(sync|destroy|template|apply|diff)\b")


def test_every_helmfile_run_in_the_justfile_names_its_concurrency():
    runs = [
        line.strip()
        for line in (ROOT / "justfile").read_text().splitlines()
        if SPAWNS.search(line) and not line.lstrip().startswith(("#", "echo"))
    ]
    assert runs, "the justfile runs helmfile"
    unbounded = [line for line in runs if "--concurrency {{helmfile_concurrency}}" not in line]
    assert unbounded == [], unbounded


@pytest.mark.skipif(shutil.which("just") is None, reason="just not installed")
def test_dev_apply_runs_helmfile_with_four_and_takes_an_override():
    def dry_run(**extra):
        env = {key: value for key, value in os.environ.items() if key != "JC_HELMFILE_CONCURRENCY"}
        env.update(JUST_NO_DOTENV="true", **extra)
        done = subprocess.run(
            ["just", "--dry-run", "dev-apply"], cwd=ROOT, env=env, capture_output=True, text=True, check=True
        )
        lines = (line.strip() for line in (done.stdout + done.stderr).splitlines())
        return [line for line in lines if line.startswith("helmfile")]

    # the selectors are empty on a full sync (T-6135)
    sync = 'helmfile -f deployment/helmfile.yaml -e dev sync --concurrency {} "${{selectors[@]}}"'
    assert dry_run() == [sync.format(4)]
    assert dry_run(JC_HELMFILE_CONCURRENCY="2") == [sync.format(2)]
