"""T-6443: the stage instance is smartcity's copy with its own names and keys, and its recipes
refuse every other cluster."""

import shutil
import subprocess

import pytest

from test_dev_recipes import DEV_SERVER, NODES, PROJECT_ROOT, guard_env

STAGE_SERVER = f"https://{NODES['stage']}:6443"
PROD_SERVER = f"https://{NODES['smartcity']}:6443"
ENVIRONMENT = PROJECT_ROOT / "ci/example-deployments/environments/stage"
# Prod's person, age recipient of the shared default, backup bucket.
OTHER_INSTANCES = ("fc3b55df-5fd4-4679-af5e-bb9dca22b0d7", "age1dwqmmfzz", "joinedcontext-prod-backup", "JC_SMARTCITY_ISSUER")


def run_guard(tmp_path, server):
    env = guard_env(tmp_path, server)
    return subprocess.run(["just", "_stage-guard"], cwd=PROJECT_ROOT, env=env, capture_output=True, text=True, check=False)


@pytest.mark.skipif(shutil.which("just") is None or shutil.which("kubectl") is None, reason="just and kubectl")
@pytest.mark.parametrize("server", [DEV_SERVER, PROD_SERVER, "https://203.0.113.3:6443", "https://1203.0.113.30:6443"])
def test_the_stage_guard_refuses_every_other_cluster(tmp_path, server):
    result = run_guard(tmp_path, server)
    assert result.returncode != 0 and "refusing" in result.stderr


@pytest.mark.skipif(shutil.which("just") is None or shutil.which("kubectl") is None, reason="just and kubectl")
def test_the_stage_guard_accepts_the_stage_cluster(tmp_path):
    result = run_guard(tmp_path, STAGE_SERVER)
    assert result.returncode == 0, result.stderr


def test_stage_names_nothing_of_another_instance():
    for path in sorted(ENVIRONMENT.glob("*.gotmpl")):
        text = path.read_text()
        assert "smartcity.joinedcontext.com" not in text, path
        assert not [word for word in OTHER_INSTANCES if word in text], path


def test_the_walker_person_is_stages_alone():
    """T-6449: only stage names a person an agent signs in as; prod keeps the owner alone."""
    assert "walkerUserId" in (ENVIRONMENT / "global.yaml.gotmpl").read_text()
    smartcity = ENVIRONMENT.parent / "smartcity"
    assert not [p for p in smartcity.glob("*.gotmpl") if "walkerUserId" in p.read_text()]
