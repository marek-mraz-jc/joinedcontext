"""The builder image and the seeded workflows agree (T-2633, AP-80, AP-82).

scripts/check-builder-contract.py is what scripts/pin-portal.sh runs against a builder image
before it pins it; these tests hold its two rules on the real workflows, with the builder of
portal 080be67 (the pin that failed every application) and of 50b8f20 (the one that runs them).
"""

import glob
import importlib.util
import pathlib

import pytest
import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("contract", ROOT / "scripts/check-builder-contract.py")
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)

WORKFLOWS = sorted(glob.glob(str(ROOT / "components/gitea/apps/*/.gitea/workflows/build.yml")))

OLD_LANE = 'throw new Error("usage: lane.mjs deps <app-dir> | functions <app-dir> <out-dir>");'
OLD_BUILD_APP = """set -eu
for name in JC_APP_REPO JC_APP_COMMIT JC_APP_BUILD JC_STORE_URL JC_STORE_BUCKET JC_STORE_PREFIX \\
  JC_FORGE_TOKEN AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY; do
  eval "[ -n \\"\\${$name:-}\\" ]" || fail "$name is not set"
done
"""
NEW_LANE = (
    '"usage: lane.mjs deps <app-dir> | functions <app-dir> <out-dir> | app <owner/repo> | '
    'sbom <node_modules> <out> | image <layer.tar> <dir> | upload <build-dir> | propose <owner/repo>",'
)
NEW_BUILD_APP = """for name in JC_APP_REPO JC_APP_COMMIT JC_FORGE_TOKEN; do
  eval "[ -n \\"\\${$name:-}\\" ]" || fail "$name is not set"
done
"""


def load(path):
    with open(path) as f:
        return yaml.safe_load(f)


def test_every_sample_app_carries_a_workflow():
    assert len(WORKFLOWS) >= 3


@pytest.mark.parametrize("path", WORKFLOWS)
def test_the_current_builder_runs_the_seeded_workflow(path):
    assert contract.problems(NEW_LANE, NEW_BUILD_APP, load(path), path) == []


@pytest.mark.parametrize("path", WORKFLOWS)
def test_the_pre_t2609_builder_is_refused(path):
    found = contract.problems(OLD_LANE, OLD_BUILD_APP, load(path), path)
    assert any("lane.mjs upload" in line for line in found)
    assert any("lane.mjs propose" in line for line in found)
    assert any("without JC_APP_BUILD" in line for line in found)
    assert not any("without JC_APP_REPO" in line for line in found), "set inline before build-app"


def test_a_builder_without_its_markers_is_refused():
    with pytest.raises(ValueError):
        contract.lane_commands("console.log('no usage here')")
    with pytest.raises(ValueError):
        contract.required_variables("#!/bin/sh\nexit 0\n")


def test_the_cli_exits_nonzero_on_a_mismatch(tmp_path):
    (tmp_path / "lane.mjs").write_text(OLD_LANE)
    (tmp_path / "build-app").write_text(NEW_BUILD_APP)
    argv = ["x", str(tmp_path / "lane.mjs"), str(tmp_path / "build-app"), WORKFLOWS[0]]
    assert contract.main(argv) == 1
    (tmp_path / "lane.mjs").write_text(NEW_LANE)
    assert contract.main(argv) == 0
