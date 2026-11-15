"""T-4795: scripts/edge-smoke.sh, the gate of ci-full's k3d upgrade job.

ci-full 39353463338 and 39362707152 ran the live installation's smoke.sh after a green k3d upgrade;
35 checks k3d cannot meet (live feeds, an Actions runner, APISIX as the outer hop) rolled it back.
The edge smoke proves what the variants harness proves after a fresh install. A curl stub on PATH
answers each host with the status the test names.
"""

import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "edge-smoke.sh"

CURL = """#!/bin/sh
for a; do url="$a"; done
case "$url" in
  *idm.*) printf '%s' "$IDM_CODE" ;;
  *portal.*) printf '%s' "$PORTAL_CODE" ;;
  *) printf '000' ;;
esac
"""


def smoke(tmp_path, idm, portal, *args):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    (bin_dir / "curl").write_text(CURL)
    (bin_dir / "curl").chmod(0o755)
    env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "IDM_CODE": idm,
           "PORTAL_CODE": portal, "JC_EDGE_SMOKE_SECONDS": "0"}
    args = args or ("https://joinedcontext.test", "https://idm.joinedcontext.test")
    return subprocess.run([str(SCRIPT), *args], check=False, capture_output=True, text=True, env=env, timeout=30)


@pytest.mark.parametrize("portal", ["200", "302", "401", "403"])
def test_green_when_keycloak_and_the_portal_answer_through_the_edge(tmp_path, portal):
    result = smoke(tmp_path, "200", portal)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "ok    the portal host answers through the edge" in result.stdout


@pytest.mark.parametrize("idm, portal, failed", [
    ("000", "302", "keycloak OIDC discovery"),     # unreachable: the red run's .invalid domain
    ("503", "302", "keycloak OIDC discovery"),
    ("200", "404", "the portal host"),             # APISIX matched no route
    ("200", "502", "the portal host"),             # broken upstream
    ("200", "4031", "the portal host"),            # the status regex is anchored
])
def test_red_on_a_gateway_error_or_no_answer(tmp_path, idm, portal, failed):
    result = smoke(tmp_path, idm, portal)
    assert result.returncode == 1
    assert f"FAIL  {failed}" in result.stderr


def test_refuses_a_url_that_is_not_https(tmp_path):
    result = smoke(tmp_path, "200", "200", "http://joinedcontext.test", "https://idm.joinedcontext.test")
    assert result.returncode == 2
    assert "is not an https URL" in result.stderr
