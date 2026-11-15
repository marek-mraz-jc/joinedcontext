"""Stateful claims in the production-shaped namespaces use the Retain class (T-6718, G-5)."""
import shutil
import subprocess

import pytest
import yaml

from conftest import PROJECT_ROOT

RETAIN_CLASS = PROJECT_ROOT / "cluster/hetzner/module/extra-manifests/05-local-path-retain.yaml.tpl"


def gate(path):
    binary = shutil.which("conftest")
    if not binary:
        pytest.skip("conftest is not installed")
    return subprocess.run([binary, "test", "--all-namespaces", "-p", str(PROJECT_ROOT / "policies"), str(path)],
                          capture_output=True, text=True)


def claim(ns, name, cls):
    spec = {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "1Gi"}}}
    if cls is not None:
        spec["storageClassName"] = cls
    return {"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": name, "namespace": ns}, "spec": spec}


def cluster(ns, name, cls):
    return {"apiVersion": "postgresql.cnpg.io/v1", "kind": "Cluster", "metadata": {"name": name, "namespace": ns},
            "spec": {"instances": 1, "storage": {"size": "1Gi", "storageClass": cls}}}


def statefulset(ns, name, cls):
    return {"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name, "namespace": ns},
            "spec": {"volumeClaimTemplates": [claim(ns, "data", cls)]}}


def test_the_retain_class_is_in_git_and_keeps_the_volume():
    sc = yaml.safe_load(RETAIN_CLASS.read_text())
    assert sc["kind"] == "StorageClass" and sc["metadata"]["name"] == "local-path-retain"
    assert sc["reclaimPolicy"] == "Retain"
    assert "05-local-path-retain.yaml" in (RETAIN_CLASS.parent / "kustomization.yaml.tpl").read_text()


def test_the_gate_refuses_a_stateful_claim_on_a_delete_class(tmp_path):
    bad = tmp_path / "bad.yaml"
    bad.write_text(yaml.safe_dump_all([claim("jc-prod", "uploads", "local-path"),
                                       claim("jc-prod-mqtt", "broker-data", None),
                                       cluster("jc-dev", "apps-db", "hcloud-volumes"),
                                       statefulset("jc-prod", "artifact-store", "local-path")]))
    result = gate(bad)
    assert result.returncode == 1
    for expected in ('jc-prod/PersistentVolumeClaim/uploads: a claim names class "local-path"',
                     'jc-prod-mqtt/PersistentVolumeClaim/broker-data: a claim names class ""',
                     'jc-dev/Cluster/apps-db: a claim names class "hcloud-volumes"',
                     'jc-prod/StatefulSet/artifact-store: a claim names class "local-path"'):
        assert expected in result.stdout, result.stdout


def test_the_gate_passes_retained_claims_and_leaves_other_namespaces_alone(tmp_path):
    good = tmp_path / "good.yaml"
    good.write_text(yaml.safe_dump_all([claim("jc-prod", "uploads", "local-path-retain"),
                                        cluster("jc-dev", "apps-db", "local-path-retain"),
                                        statefulset("jc-prod", "artifact-store", "local-path-retain"),
                                        claim("jc-prod", "jc-files", "local-path"),
                                        claim("dev", "scratch", "local-path")]))
    result = gate(good)
    assert result.returncode == 0, result.stdout
