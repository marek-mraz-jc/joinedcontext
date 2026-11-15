"""Releases whose post-upgrade Job outlasts Helm's 300 s default carry their own timeout.

Helm waits for a hook Job and fails the apply when its timeout passes, though the Job then
succeeds: gitea's bootstrap seeds six projects in about 7 min (T-3207), Keycloak's admin
permissions link every endpoint: scope to mcp-hub in about 8 min on a fresh realm (T-3324)."""

import shutil
import subprocess
from pathlib import Path

import pytest
from conftest import load_yaml_all

PROJECT_ROOT = Path(__file__).resolve().parents[1]

SLOW_HOOKS = {"gitea-bootstrap": 900, "keycloak-admin-permissions": 900}


@pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")
def test_slow_hook_releases_wait_for_their_job():
    if not (PROJECT_ROOT / "deployment/environments/dev").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    built = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "dev", "build"],
        cwd=PROJECT_ROOT, capture_output=True, text=True,
    )
    assert built.returncode == 0, built.stderr
    releases = {
        r["name"]: r
        for s in load_yaml_all(built.stdout) if isinstance(s, dict)
        for r in s.get("releases") or []
    }
    for name, least in SLOW_HOOKS.items():
        assert name in releases, f"no release {name} on dev"
        assert releases[name].get("timeout", 0) >= least, f"{name}: {releases[name].get('timeout')}"
