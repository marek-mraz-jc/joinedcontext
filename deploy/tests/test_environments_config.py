import subprocess
import shutil
from typing import Any, Dict, Optional

import pytest
import yaml
from conftest import load_yaml_all

# Writes `deployment/environments/testing` in its own copy of the tree (`own_tree`), so it
# shares nothing with another module. Its own xdist group keeps the module on one worker, so its
# module-scoped renders happen once rather than once per worker (T-2895).
pytestmark = pytest.mark.xdist_group("environments-config")

@pytest.fixture
def deployment_dir(own_tree):
    return own_tree / "deployment"

@pytest.fixture
def render_helmfile(deployment_dir):
    def _render_helmfile(*, use_test_environment: bool)  -> str:
        result = subprocess.run(
            ["helmfile", "template", "-f", "helmfile.yaml", "--skip-deps"] + (["-e", "testing"] if use_test_environment else []),
            cwd=deployment_dir,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            pytest.fail(
                "helmfile template failed with exit code "
                f"{result.returncode}\nSTDOUT:\n{result.stdout}\nSTDERR:\n{result.stderr}"
            )
        return result.stdout
    return _render_helmfile

@pytest.fixture
def configure_environment(deployment_dir):
    env_dir = deployment_dir / "environments" / "testing"
    if env_dir.exists():
        shutil.rmtree(env_dir)
    env_dir.mkdir(parents=True)

    def set_value(target, dotted_path, value):
        keys = dotted_path.split(".")
        cursor = target
        for key in keys[:-1]:
            cursor = cursor.setdefault(key, {})
        cursor[keys[-1]] = value

    def _configure_environment(env_vars):
        configured_global = False
        for filename, mappings in env_vars.items():
            file_path = env_dir / filename
            data = {}
            if file_path.exists():
                existing = yaml.safe_load(file_path.read_text()) or {}
                if isinstance(existing, dict):
                    data = existing
            for yaml_path, yaml_value in mappings.items():
                set_value(data, yaml_path, yaml_value)
            file_path.write_text(yaml.safe_dump(data, sort_keys=False))
            if filename == "global.yaml.gotmpl":
                configured_global = True
        if not configured_global:
            (env_dir / "global.yaml.gotmpl").write_text("# No custom global values configured.\n")

    yield _configure_environment

    if env_dir.exists():
        shutil.rmtree(env_dir)

def _normalize_path(path: Any) -> list[str]:
    if isinstance(path, tuple):
        return list(path)
    return str(path).split(".")

def _get_by_path(document: Dict[str, Any], path: str):
    cursor: Any = document
    for part in _normalize_path(path):
        if isinstance(cursor, dict):
            cursor = cursor.get(part)
        elif isinstance(cursor, list) and part.isdigit():
            idx = int(part)
            cursor = cursor[idx] if 0 <= idx < len(cursor) else None
        else:
            return None
        if cursor is None:
            return None
    return cursor

def assert_contains_k8s_resource(
    rendered_output: str,
    *,
    match: Dict[str | tuple, Any],
    asserts: Optional[Dict[str, Any]] = None,
):
    documents = [
        doc for doc in load_yaml_all(rendered_output)
        if isinstance(doc, dict)
    ]
    for doc in documents:
        if all(_get_by_path(doc, path) == expected for path, expected in match.items()):
            for path, expected in (asserts or {}).items():
                actual = _get_by_path(doc, path)
                if actual != expected:
                    raise AssertionError(
                        f"Resource matched {match} but {path} expected {expected!r}, actual {actual!r}"
                    )
            return doc
    raise AssertionError(f"Did not find resource matching {match}")

def label(key: str) -> tuple:
    return "metadata", "labels", key

def test_default_environment_is_sufficient(render_helmfile):
    render_helmfile(use_test_environment=False)

def test_can_overwrite_global_values(configure_environment, render_helmfile):
    configure_environment({
        "global.yaml.gotmpl": {
            "global.domain": "mycustomdomain.com"
        }
    })
    output = render_helmfile(use_test_environment=True)
    assert "mycustomdomain.com" in output

APISIX_SA = {
    "kind": "ServiceAccount",
    ("metadata", "name"): "apisix",
}

def test_can_overwrite_component_global_values(configure_environment, render_helmfile):
    configure_environment({
        "apisix.yaml.gotmpl": {
            "apisix.apisix.namespace": "my-new-namespace"
        }
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match=APISIX_SA,
        asserts={"metadata.namespace": "my-new-namespace"},
    )

def test_can_overwrite_component_helm_values(configure_environment, render_helmfile):
    configure_environment({
        "apisix.yaml.gotmpl": {
            "apisix.apisix.rawValues.replicaCount": 5
        }
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match={"kind": "Deployment", ("metadata", "name"): "apisix"},
        asserts={"spec.replicas": 5},
    )


def test_multi_namespace_different_components_use_distinct_namespaces(configure_environment, render_helmfile):
    configure_environment({
        "global.yaml.gotmpl": {
            "global.singleNamespace": False,
            "global.instanceSlug": "dev",
        }
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match=APISIX_SA,
        asserts={"metadata.namespace": "dev-apisix"},
    )
    assert_contains_k8s_resource(
        output,
        match={"kind": "Database"},
        asserts={"metadata.namespace": "dev-postgres"},
    )


def test_single_namespace_all_components_colocated(configure_environment, render_helmfile):
    configure_environment({
        "global.yaml.gotmpl": {
            "global.singleNamespace": True,
            "global.instanceSlug": "shared",
        }
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match=APISIX_SA,
        asserts={"metadata.namespace": "shared"},
    )
    assert_contains_k8s_resource(
        output,
        match={"kind": "Database"},
        asserts={"metadata.namespace": "shared"},
    )


def test_per_component_namespace_override_applies_in_single_namespace_mode(configure_environment, render_helmfile):
    configure_environment({
        "global.yaml.gotmpl": {
            "global.singleNamespace": True,
        },
        "apisix.yaml.gotmpl": {
            "apisix.apisix.namespace": "special-apisix",
        },
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match=APISIX_SA,
        asserts={"metadata.namespace": "special-apisix"},
    )


def test_per_component_override_affects_only_that_component(configure_environment, render_helmfile):
    configure_environment({
        "global.yaml.gotmpl": {
            "global.singleNamespace": False,
            "global.instanceSlug": "dev",
        },
        "apisix.yaml.gotmpl": {
            "apisix.apisix.namespace": "special",
        },
    })
    output = render_helmfile(use_test_environment=True)
    assert_contains_k8s_resource(
        output,
        match=APISIX_SA,
        asserts={"metadata.namespace": "special"},
    )
    assert_contains_k8s_resource(
        output,
        match={"kind": "Database"},
        asserts={"metadata.namespace": "dev-postgres"},
    )
