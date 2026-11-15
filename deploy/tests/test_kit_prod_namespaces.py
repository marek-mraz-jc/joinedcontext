"""T-6719, G-1: the production-shaped namespaces enforce the restricted Pod Security Standard.

The labels live in the cluster module (helmfile creates jc-prod and jc-dev without any), and
every pod a jc-prod or jc-dev sync installs must pass that standard, or the apply stops at
admission on the cluster instead of here."""

from pathlib import Path

import pytest
import yaml

KIT = Path(__file__).resolve().parent.parent / "cluster/hetzner/module/extra-manifests"
MANIFEST = KIT / "06-prod-namespaces.yaml.tpl"
NAMESPACES = {"jc-prod", "jc-prod-mqtt", "jc-dev", "prod-final-backup"}


def test_the_four_namespaces_enforce_restricted_at_the_latest_version():
    text = MANIFEST.read_text()
    assert "${" not in text and "%{" not in text  # kube-hetzner's templatefile() leaves it as is
    docs = [d for d in yaml.safe_load_all(text) if d]
    assert {d["metadata"]["name"] for d in docs} == NAMESPACES
    for ns in docs:
        labels = ns["metadata"]["labels"]
        assert labels["pod-security.kubernetes.io/enforce"] == "restricted", ns["metadata"]["name"]
        assert labels["pod-security.kubernetes.io/enforce-version"] == "latest"
        assert labels["pod-security.kubernetes.io/warn"] == labels["pod-security.kubernetes.io/audit"] == "restricted"
    assert "06-prod-namespaces.yaml" in (KIT / "kustomization.yaml.tpl").read_text()


def pod_spec(doc):
    spec = doc.get("spec") or {}
    if doc["kind"] in ("Deployment", "StatefulSet", "DaemonSet", "Job", "ReplicaSet"):
        return spec["template"]["spec"]
    if doc["kind"] == "CronJob":
        return spec["jobTemplate"]["spec"]["template"]["spec"]
    if doc["kind"] == "Pod":
        return spec
    return None


def restricted_breaches(pod):
    """The restricted profile's checks that a chart can get wrong (kubernetes.io/docs pod-security-standards)."""
    breaches = []
    if pod.get("hostNetwork") or pod.get("hostPID") or pod.get("hostIPC"):
        breaches.append("host namespaces")
    if any("hostPath" in v for v in pod.get("volumes") or []):
        breaches.append("hostPath")
    level = pod.get("securityContext") or {}
    for c in (pod.get("initContainers") or []) + pod["containers"]:
        sc = c.get("securityContext") or {}
        caps = sc.get("capabilities") or {}
        if sc.get("privileged"):
            breaches.append(f"{c['name']}: privileged")
        if not sc.get("runAsNonRoot", level.get("runAsNonRoot")):
            breaches.append(f"{c['name']}: runAsNonRoot")
        if (sc.get("seccompProfile") or level.get("seccompProfile") or {}).get("type") not in ("RuntimeDefault", "Localhost"):
            breaches.append(f"{c['name']}: seccompProfile")
        if sc.get("allowPrivilegeEscalation") is not False:
            breaches.append(f"{c['name']}: allowPrivilegeEscalation")
        if "ALL" not in (caps.get("drop") or []) or set(caps.get("add") or []) - {"NET_BIND_SERVICE"}:
            breaches.append(f"{c['name']}: capabilities")
        if any(p.get("hostPort") for p in c.get("ports") or []):
            breaches.append(f"{c['name']}: hostPort")
    return breaches


@pytest.mark.parametrize("env", ["jc-prod", "jc-dev"])
def test_every_pod_a_sync_installs_passes_restricted(rendered, env):
    refused = {}
    for doc in rendered(env):
        if not doc or doc.get("metadata", {}).get("namespace") not in NAMESPACES:
            continue
        # A `helm test` hook runs only under `helm test`, never in a sync.
        if (doc["metadata"].get("annotations") or {}).get("helm.sh/hook") == "test":
            continue
        pod = pod_spec(doc)
        if pod and (breaches := restricted_breaches(pod)):
            refused[f"{doc['kind']}/{doc['metadata']['name']}"] = breaches
    assert not refused, f"admission would refuse these in {env}: {refused}"
