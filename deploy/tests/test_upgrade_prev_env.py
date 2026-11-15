"""T-4784: the upgrade job's starting environment, rebuilt from the previous release's harness.

`scripts/test-deployment-variants.sh` removes its generated scratch environment on exit, with
--keep too, so ci-full 39335432708 failed copying `environments/testing` out of the previous
release's worktree. `scripts/prev-env-overlay.sh` writes it again with that harness's own
write_env_overlay.
"""

import subprocess
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "prev-env-overlay.sh"
HARNESS = ROOT / "scripts" / "test-deployment-variants.sh"


def run(harness: Path, out: Path) -> subprocess.CompletedProcess:
    return subprocess.run([str(SCRIPT), str(harness), str(out)], capture_output=True, text=True)


def test_writes_the_0_0_0_environment_the_harness_installs(tmp_path):
    out = tmp_path / "environments" / "testing" / "global.yaml.gotmpl"
    r = run(HARNESS, out)
    assert r.returncode == 0, r.stderr
    g = yaml.safe_load(out.read_text())["global"]
    assert g["profile"] == "development"
    assert g["domain"] == "joinedcontext.test"
    assert g["instanceSlug"] == "dev"
    assert g["singleNamespace"] is True
    assert g["serviceMesh"]["enable"] is False
    assert yaml.safe_load(out.read_text())["componentProfile"] == "full"  # T-4856


def test_a_harness_without_the_overlay_is_refused(tmp_path):
    fake = tmp_path / "harness.sh"
    fake.write_text('PROFILE="development"\n')
    out = tmp_path / "global.yaml.gotmpl"
    r = run(fake, out)
    assert r.returncode == 1
    assert "no write_env_overlay" in r.stderr
    assert not out.exists()


def test_the_upgrade_job_never_reads_the_removed_scratch_environment():
    text = (ROOT / "scripts" / "ci-upgrade-e2e.sh").read_text()
    assert "prev/deployment/environments" not in text
    assert "prev-env-overlay.sh" in text
