"""T-0232: the dev-cluster recipes must never touch a foreign cluster."""

import os
import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
# TEST-NET-3 addresses (RFC 5737) for the guards; the real node addresses live only in the private
# environments' nodes.yaml (T-6982), which `just _node-ip` reads from JC_ENVIRONMENTS.
NODES = {"dev": "203.0.113.10", "smartcity": "203.0.113.20", "stage": "203.0.113.30"}
DEV_SERVER = f"https://{NODES['dev']}:6443"
FOREIGN_SERVER = "https://10.11.12.13:6443"

requires_just = pytest.mark.skipif(shutil.which("just") is None, reason="just not installed")
requires_kubectl = pytest.mark.skipif(shutil.which("kubectl") is None, reason="kubectl not installed")


def kubeconfig(tmp_path: Path, server: str) -> Path:
    path = tmp_path / "kubeconfig.yaml"
    path.write_text(
        textwrap.dedent(
            f"""\
            apiVersion: v1
            kind: Config
            current-context: ctx
            clusters:
              - name: c
                cluster:
                  server: {server}
            contexts:
              - name: ctx
                context: {{cluster: c, user: u}}
            users:
              - name: u
                user: {{token: not-a-real-token}}
            """
        )
    )
    return path


def write_nodes(environments: Path) -> Path:
    """An environments tree holding only the test nodes.yaml; returns its path."""
    environments.mkdir(parents=True, exist_ok=True)
    (environments / "nodes.yaml").write_text("".join(f"{name}: {ip}\n" for name, ip in NODES.items()))
    return environments


def guard_env(tmp_path: Path, server: str) -> dict:
    """The environment a guard runs in: a kube context at `server`, the test node addresses."""
    return dict(os.environ, KUBECONFIG=str(kubeconfig(tmp_path, server)),
                JC_ENVIRONMENTS=str(write_nodes(tmp_path / "environments")))


def run_guard(tmp_path: Path, server: str) -> subprocess.CompletedProcess:
    env = guard_env(tmp_path, server)
    return subprocess.run(
        ["just", "_dev-guard"], cwd=PROJECT_ROOT, env=env, capture_output=True, text=True, check=False
    )


@requires_just
@requires_kubectl
def test_guard_aborts_on_a_foreign_context(tmp_path):
    result = run_guard(tmp_path, FOREIGN_SERVER)
    assert result.returncode != 0
    assert "refusing" in result.stderr


@requires_just
@requires_kubectl
def test_guard_accepts_the_dev_cluster(tmp_path):
    result = run_guard(tmp_path, DEV_SERVER)
    assert result.returncode == 0, result.stderr


@requires_just
@pytest.mark.parametrize("recipe", ["dev-apply", "dev-apply-pins", "dev-apply-pins abc true", "dev-smoke", "dev-validate", "dev-restore-drill", "dev-chaos", "dev-destroy",
                                    "dev-publish-health s.json 1", "dev-publish-health s.json 1 true",
                                    "dev-rotate-client", "dev-rotate-db ckan true", "dev-rotate-forge-token",
                                    "dev-rotate-ckan-token true", "dev-rotate-model-key key.txt true",
                                    "dev-rotation-drill", "dev-rotation-drill key.txt true"])
def test_recipe_is_parseable(recipe):
    result = subprocess.run(
        ["just", "--dry-run", *recipe.split()], cwd=PROJECT_ROOT, capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


def test_smoke_script_requires_both_urls():
    result = subprocess.run(
        ["./scripts/smoke.sh"], cwd=PROJECT_ROOT, capture_output=True, text=True, check=False
    )
    assert result.returncode != 0
    assert "usage" in result.stderr


# T-0388: a meshed ingress controller must also mark 443 opaque, or a meshed client that
# sends its own TLS through it has the stream parsed as HTTP and reset.
MESH_INGRESS_SOURCES = ("justfile", "scripts/test-deployment-variants.sh")


@pytest.mark.parametrize("source", MESH_INGRESS_SOURCES)
def test_every_place_that_meshes_the_ingress_marks_443_opaque(source):
    text = (PROJECT_ROOT / source).read_text(encoding="utf-8")
    if "linkerd.io/inject=enabled" not in text:
        pytest.skip(f"{source} does not mesh a namespace")
    assert "config.linkerd.io/opaque-ports=443" in text, (
        f"{source} meshes the ingress controller without marking 443 opaque: the Portal "
        "resolves the public issuer host onto that controller, so its own HTTPS reaches a "
        "meshed destination on 443 and Linkerd's protocol detection resets it"
    )


def test_dev_destroy_deletes_only_the_platform_namespaces():
    """T-5702 (F-DEP2-013): the namespace pattern lacked an end anchor on `dev`, so `devops` or
    any namespace starting with "dev" lost its PVCs and itself."""
    recipe = (PROJECT_ROOT / "justfile").read_text().split("\ndev-destroy:\n", 1)[1].split("\n\n", 1)[0]
    patterns = set(re.findall(r"grep -E '([^']+)'", recipe))
    assert len(patterns) == 1, patterns
    pattern = re.compile(patterns.pop())
    for ns in ("dev", "dev-portal", "dev-x-apps", "jc-operators"):
        assert pattern.search(ns), ns
    for ns in ("devops", "developer", "jc-operators-old", "kube-system", "pv-dev"):
        assert not pattern.search(ns), ns
