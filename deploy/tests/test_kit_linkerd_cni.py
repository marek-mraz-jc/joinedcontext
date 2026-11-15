"""T-5327: the cluster module's Linkerd CNI HelmChart keeps the values dev and prod run.

A node rebuilt without this chart meshes pods with linkerd-init
(NET_ADMIN), which the Portal's PSS-restricted App namespaces refuse."""

from pathlib import Path

import yaml

KIT = Path(__file__).resolve().parent.parent / "cluster/hetzner/module"
MANIFEST = KIT / "extra-manifests/25-linkerd-cni.yaml.tpl"


def _docs():
    # kube-hetzner renders *.yaml.tpl with templatefile(); this one has no ${...}, so it is plain YAML.
    text = MANIFEST.read_text()
    assert "${" not in text and "%{" not in text
    return {d["kind"]: d for d in yaml.safe_load_all(text)}


def test_namespace_admits_the_privileged_daemonset_and_stays_out_of_the_mesh():
    ns = _docs()["Namespace"]
    assert ns["metadata"]["name"] == "linkerd-cni"
    assert ns["metadata"]["labels"]["pod-security.kubernetes.io/enforce"] == "privileged"
    assert ns["metadata"]["labels"]["config.linkerd.io/admission-webhooks"] == "disabled"
    assert ns["metadata"]["annotations"]["linkerd.io/inject"] == "disabled"


def test_helmchart_is_pinned_and_carries_the_k3s_values():
    chart = _docs()["HelmChart"]
    spec = chart["spec"]
    assert (chart["metadata"]["namespace"], spec["chart"], spec["targetNamespace"]) == (
        "kube-system", "linkerd2-cni", "linkerd-cni")
    assert spec["repo"] == "https://helm.linkerd.io/edge"
    assert spec["version"] == "2026.10.1"
    values = yaml.safe_load(spec["valuesContent"])
    assert values["destCNINetDir"] == "/var/lib/rancher/k3s/agent/etc/cni/net.d"
    assert values["destCNIBinDir"] == "/var/lib/rancher/k3s/data/cni"
    assert values["privileged"] is True
    assert values["repairController"]["enabled"] is True
    assert values["repairController"]["resources"]["memory"]["limit"] == "128Mi"


def test_the_control_plane_turns_it_on_and_the_kustomization_lists_it():
    assert "cniEnabled: true" in (KIT / "extra-manifests/30-linkerd.yaml.tpl").read_text()
    kustomization = (KIT / "extra-manifests/kustomization.yaml.tpl").read_text()
    assert kustomization.index("- 25-linkerd-cni.yaml") < kustomization.index("- 30-linkerd.yaml")
