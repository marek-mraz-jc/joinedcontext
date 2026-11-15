"""T-5552: `just golden-update` renders the environments in parallel and fails by name.

The recipe runs in a copy of the repository whose scripts/render.sh and helmfile are stubs:
a render sleeps, records how many renders were running beside it and writes one ConfigMap
named after its environment, so each golden file must list exactly that object.
"""

import os
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
ENVIRONMENTS = (PROJECT_ROOT / "ci/golden/ENVIRONMENTS").read_text().split()

requires_just = pytest.mark.skipif(shutil.which("just") is None, reason="just not installed")

RENDER_STUB = textwrap.dedent(
    """\
    #!/usr/bin/env bash
    set -eu
    env="$1" out="$2"
    log="$STUB_DIR/running"
    mkdir -p "$log"
    touch "$log/$env"
    ls "$log" | wc -l >> "$STUB_DIR/concurrency"
    echo "$env" >> "$STUB_DIR/rendered"
    sleep 0.5
    rm "$log/$env"
    if [ "$env" = "${STUB_FAIL:-}" ]; then echo "stub: template error in $env" >&2; exit 1; fi
    printf 'apiVersion: v1\\nkind: ConfigMap\\nmetadata:\\n  name: golden-%s\\n  namespace: ns\\n' "$env" > "$out"
    """
)

HELMFILE_STUB = textwrap.dedent(
    """\
    #!/usr/bin/env bash
    echo "$*" >> "$STUB_DIR/helmfile"
    case "$*" in
      *" template "*) printf 'apiVersion: v1\\nkind: ConfigMap\\nmetadata:\\n  name: demo\\n  namespace: ns\\n' ;;
    esac
    """
)


def repo_copy(tmp_path: Path) -> tuple[Path, Path]:
    root = tmp_path / "repo"
    (root / "scripts").mkdir(parents=True)
    (root / "ci/golden").mkdir(parents=True)
    (root / "defaults/deployment").mkdir(parents=True)
    (root / "ci/example-deployments/environments").mkdir(parents=True)
    (root / "ci/example-deployments/helmfile.yaml").write_text("")
    shutil.copy(PROJECT_ROOT / "justfile", root)
    shutil.copy(PROJECT_ROOT / "scripts/render-all.sh", root / "scripts")
    for name in ("ENVIRONMENTS", "list-objects.py"):
        shutil.copy(PROJECT_ROOT / "ci/golden" / name, root / "ci/golden")
    (root / "scripts/render.sh").write_text(RENDER_STUB)
    (root / "scripts/render.sh").chmod(0o755)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "helmfile").write_text(HELMFILE_STUB)
    (bin_dir / "helmfile").chmod(0o755)
    return root, bin_dir


def golden_update(tmp_path: Path, **extra: str) -> tuple[Path, subprocess.CompletedProcess]:
    root, bin_dir = repo_copy(tmp_path)
    stub_dir = tmp_path / "stub"
    stub_dir.mkdir()
    env = dict(
        os.environ,
        PATH=f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
        STUB_DIR=str(stub_dir),
        JUST_NO_DOTENV="true",
        **extra,
    )
    result = subprocess.run(
        ["just", "golden-update"], cwd=root, env=env, capture_output=True, text=True, check=False, timeout=120
    )
    return root, result


@requires_just
def test_every_environment_renders_in_parallel_into_its_own_golden(tmp_path):
    root, result = golden_update(tmp_path)
    assert result.returncode == 0, result.stderr
    stub_dir = tmp_path / "stub"
    assert sorted((stub_dir / "rendered").read_text().split()) == sorted(ENVIRONMENTS)
    for env in ENVIRONMENTS:
        assert (root / f"ci/golden/{env}.txt").read_text() == f"v1/ConfigMap ns/golden-{env}\n"
    # Default GOLDEN_JOBS=3: several renders overlap, never more than three.
    peaks = [int(n) for n in (stub_dir / "concurrency").read_text().split()]
    assert 2 <= max(peaks) <= 3, peaks
    # helmfile repos ran for every environment before the renders.
    repos = [line for line in (stub_dir / "helmfile").read_text().splitlines() if line.endswith(" repos")]
    assert len(repos) == len(ENVIRONMENTS)


@requires_just
def test_golden_jobs_one_renders_one_after_another(tmp_path):
    _, result = golden_update(tmp_path, GOLDEN_JOBS="1")
    assert result.returncode == 0, result.stderr
    assert set((tmp_path / "stub/concurrency").read_text().split()) == {"1"}


@requires_just
def test_a_failed_render_fails_the_recipe_and_names_its_environment(tmp_path):
    failing = ENVIRONMENTS[len(ENVIRONMENTS) // 2]
    root, result = golden_update(tmp_path, STUB_FAIL=failing)
    assert result.returncode != 0
    assert f"render failed for: {failing}" in result.stderr
    assert f"stub: template error in {failing}" in result.stderr
    # The other renders still ran, and no golden was rewritten from a partial set.
    assert sorted((tmp_path / "stub/rendered").read_text().split()) == sorted(ENVIRONMENTS)
    assert not list((root / "ci/golden").glob("*.txt"))


def render_all(tmp_path: Path, environment: str, **extra: str) -> subprocess.CompletedProcess:
    root, bin_dir = repo_copy(tmp_path)
    env = dict(os.environ, PATH=f"{bin_dir}{os.pathsep}{os.environ['PATH']}", **extra)
    return subprocess.run(
        ["scripts/render-all.sh", str(tmp_path / "out"), environment],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.parametrize("jobs", ["0", "x", "-1"])
def test_render_all_refuses_a_bad_job_count(tmp_path, jobs):
    result = render_all(tmp_path, "dev", RENDER_JOBS=jobs)
    assert result.returncode == 2
    assert "RENDER_JOBS" in result.stderr


def test_render_all_refuses_a_path_as_environment(tmp_path):
    result = render_all(tmp_path, "../x")
    assert result.returncode == 2
    assert "bad environment name" in result.stderr
