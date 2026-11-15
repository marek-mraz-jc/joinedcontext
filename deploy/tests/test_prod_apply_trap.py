"""T-5791: `just prod-apply`'s EXIT trap ends a green apply with exit 0 and the probe summary.

The test runs the recipe's own trap line under its `set -euo pipefail` with a background
child standing in for the assistant probe: still running (the trap kills it, `wait` returns
143) and already finished (`kill` returns 1). A failing apply keeps its non-zero exit.
"""

import re
import subprocess
from pathlib import Path

import pytest

JUSTFILE = (Path(__file__).resolve().parent.parent / "justfile").read_text()


def prod_apply_trap() -> str:
    recipe = re.search(r"^prod-apply:\n((?:\t.*\n|\n)+)", JUSTFILE, re.MULTILINE)
    assert recipe, "prod-apply recipe not found in the justfile"
    traps = [
        line.strip()
        for line in recipe.group(1).splitlines()
        if line.strip().startswith("trap ")
    ]
    assert len(traps) == 1, traps
    return traps[0]


@pytest.mark.parametrize(
    "probe_seconds", [60, 0], ids=["probe-running", "probe-finished"]
)
@pytest.mark.parametrize(
    ("apply", "code"), [("true", 0), ("false", 1)], ids=["green", "red"]
)
def test_the_trap_prints_the_summary_and_keeps_the_apply_exit(
    tmp_path, probe_seconds, apply, code
):
    probe_log = tmp_path / "probe.log"
    script = "\n".join(
        [
            "set -euo pipefail",
            f"probe_log={probe_log}",
            'echo "assistant probe: 3 of 3 answered" > "$probe_log.summary"',
            f"sleep {probe_seconds} &",
            "probe=$!",
            "sleep 0.2",
            prod_apply_trap(),
            apply,
        ]
    )
    run = subprocess.run(
        ["bash", "-c", script], check=False, capture_output=True, text=True, timeout=30
    )
    assert run.returncode == code, run.stderr
    assert "assistant probe: 3 of 3 answered" in run.stdout
    assert f"assistant probe log: {probe_log}" in run.stdout
