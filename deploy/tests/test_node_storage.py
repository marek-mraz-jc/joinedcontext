"""T-3431: on dev the local-path helper, and only it, runs as spc_t so a released volume deletes.

The node is MicroOS with SELinux enforcing; the stock helper could not remove a build cache a
Portal build pod had written under its own MCS categories, and five caches leaked.
"""

import pytest
import yaml


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def config(docs: list[dict]) -> list[dict]:
    return [
        d for d in docs
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "local-path-config"
    ]


def test_dev_helper_pod_is_spc_t_and_pinned(dev):
    (cm,) = config(dev)
    assert cm["metadata"]["namespace"] == "kube-system"
    pod = yaml.safe_load(cm["data"]["helperPod.yaml"])
    (container,) = pod["spec"]["containers"]
    assert container["securityContext"] == {"seLinuxOptions": {"type": "spc_t"}}
    assert "@sha256:" in container["image"]
    assert "securityContext" not in pod["spec"], "the type belongs to the helper container only"


def test_dev_keeps_the_stock_provisioner_data(dev):
    (cm,) = config(dev)
    assert '"/var/lib/rancher/k3s/storage"' in cm["data"]["config.json"]
    assert 'rm -rf "${VOL_DIR}"' in cm["data"]["teardown"]
    assert 'mkdir -m 0777 -p "${VOL_DIR}"' in cm["data"]["setup"]


def test_spc_t_appears_nowhere_else_on_dev(dev):
    others = [
        f'{d["kind"]}/{d["metadata"]["name"]}'
        for d in dev
        if "spc_t" in yaml.safe_dump(d) and d not in config(dev)
    ]
    assert others == []


def test_off_outside_dev(rendered):
    assert config(rendered("local")) == []


def test_nothing_else_lands_in_or_targets_kube_system_on_dev(dev):
    # A part `namespace` of kube-system once rendered default-deny-node-storage there, which would
    # have cut CoreDNS off, and put kube-system in the prepare hook's mesh injection.
    # node-exporter is the one other node agent there (T-5154, components/prometheus): its
    # release renders it with namespaceOverride, never a part `namespace`.
    exporter = "node-exporter"
    placed = [
        f'{d["kind"]}/{d["metadata"]["name"]}'
        for d in dev
        if d["metadata"].get("namespace") == "kube-system" and d["metadata"]["name"] != exporter
    ]
    assert placed == ["ConfigMap/local-path-config"]
    matched = [
        d["metadata"]["name"]
        for d in dev
        if d.get("kind") == "ClusterPolicy"
        and "kube-system" in yaml.safe_dump([r.get("match") for r in d["spec"].get("rules", [])])
    ]
    assert matched == [], "a Kyverno policy matches kube-system"
