"""scripts/render.sh retries a chart pull that hit a network error, and nothing else (T-5183)."""
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
RESET = (
    'Error: Get "https://release-assets.githubusercontent.com/apisix-2.15.0.tgz": '
    "read tcp 10.1.0.125:37282->185.199.109.133:443: read: connection reset by peer"
)


def render(tmp_path, failures: list[str]):
    """Run render.sh with a fake helmfile that prints each failure in turn, then succeeds."""
    if shutil.which("bash") is None:
        pytest.skip("bash not installed")
    bindir = tmp_path / "bin"
    bindir.mkdir()
    (tmp_path / "calls").write_text("")
    for i, msg in enumerate(failures):
        (tmp_path / f"fail-{i}").write_text(msg)
    fake = bindir / "helmfile"
    fake.write_text(
        "#!/usr/bin/env bash\n"
        f'd="{tmp_path}"\n'
        'n=$(wc -l < "$d/calls"); echo x >> "$d/calls"\n'
        'if [ -f "$d/fail-$n" ]; then cat "$d/fail-$n" >&2; exit 1; fi\n'
        "echo 'kind: ConfigMap'\n"
    )
    fake.chmod(0o755)
    out = tmp_path / "out.yaml"
    env = {**os.environ, "PATH": f"{bindir}:{os.environ['PATH']}"}
    result = subprocess.run(
        [str(ROOT / "scripts/render.sh"), "dev", str(out)],
        check=False, cwd=str(ROOT), env=env, capture_output=True, text=True,
    )
    calls = len((tmp_path / "calls").read_text().splitlines())
    return result, out, calls


def test_a_connection_reset_while_pulling_a_chart_is_retried(tmp_path):
    result, out, calls = render(tmp_path, [RESET])
    assert result.returncode == 0, result.stderr
    assert calls == 2
    assert out.read_text() == "kind: ConfigMap\n"
    assert "attempt 1 of 3 failed" in result.stderr


def test_a_template_error_fails_at_once(tmp_path):
    result, _, calls = render(tmp_path, ["Error: template: apisix/values.yaml: nil pointer"])
    assert result.returncode == 1
    assert calls == 1
    assert "nil pointer" in result.stderr


def test_a_chart_host_that_stays_down_fails_after_three_attempts(tmp_path):
    result, _, calls = render(tmp_path, [RESET, RESET, RESET, RESET])
    assert result.returncode == 1
    assert calls == 3
    assert result.stderr.count("connection reset by peer") == 3
