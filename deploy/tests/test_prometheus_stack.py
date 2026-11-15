"""The prometheus component on dev and smartcity (T-5154): a Prometheus that keeps 31 days, an
Alertmanager, node-exporter and kube-state-metrics, every image by digest, sized requests and
limits, nothing at the edge, and every target's policy admitting it."""

import pathlib
import re

import pytest
import yaml


@pytest.fixture(scope="module", params=["dev", "smartcity"])
def env(request, rendered):
    return rendered(request.param)


def of_kind(docs, kind):
    return [d for d in docs if d.get("kind") == kind]


def one(docs, kind):
    found = of_kind(docs, kind)
    assert len(found) == 1, f"{len(found)} {kind} rendered"
    return found[0]


def pods(docs):
    for d in docs:
        if d.get("kind") in ("Deployment", "DaemonSet", "StatefulSet") and d["metadata"]["name"].startswith(("prometheus", "node-exporter")):
            yield d["metadata"]["name"], d["spec"]["template"]["spec"]


def test_prometheus_keeps_31_days_on_a_sized_volume_and_reads_every_monitor(env):
    spec = one(env, "Prometheus")["spec"]
    assert spec["retention"] == "31d"
    assert spec["retentionSize"] == "8GB"
    claim = spec["storage"]["volumeClaimTemplate"]["spec"]
    assert claim["resources"]["requests"]["storage"] == "10Gi"
    # The monitoring component's monitors carry no release label: an empty selector reads them all.
    for selector in ("serviceMonitorSelector", "podMonitorSelector", "ruleSelector", "probeSelector"):
        assert spec[selector] == {}, f"{selector} narrows to {spec[selector]}"
    assert spec["resources"]["limits"]["memory"] and spec["resources"]["requests"]["memory"]
    assert one(env, "Alertmanager")["spec"]["automountServiceAccountToken"] is False


def test_prometheus_caps_each_scrape_so_a_series_jump_cannot_oom_it(env):
    spec = one(env, "Prometheus")["spec"]
    assert spec["enforcedSampleLimit"] == 40000
    assert spec["enforcedTargetLimit"] == 200
    assert spec["enforcedLabelLimit"] == 64
    assert spec["enforcedLabelNameLengthLimit"] == 128
    assert spec["enforcedLabelValueLengthLimit"] == 1024
    assert spec["enforcedBodySizeLimit"] == "32MB"


def test_host_path_exception_matches_only_kube_system_node_exporter():
    path = pathlib.Path(__file__).resolve().parents[1] / "components/prometheus/policy-exceptions.yaml"
    (exception,) = [d for d in yaml.safe_load_all(path.read_text()) if d]
    assert exception["spec"]["match"]["any"] == [
        {"resources": {"kinds": ["DaemonSet"], "namespaces": ["kube-system"], "names": ["node-exporter"]}}
    ]


def test_every_image_is_pinned_by_digest(env):
    digest = re.compile(r"@sha256:[0-9a-f]{64}$")
    images = [one(env, "Prometheus")["spec"]["image"], one(env, "Alertmanager")["spec"]["image"]]
    for name, spec in pods(env):
        images += [c["image"] for c in spec["containers"]]
        for arg in (a for c in spec["containers"] for a in c.get("args", [])):
            if arg.startswith("--prometheus-config-reloader="):
                images.append(arg.split("=", 1)[1])
    assert len(images) >= 6
    assert [i for i in images if not digest.search(i)] == []


def test_every_container_has_requests_and_a_memory_limit(env):
    for name, spec in pods(env):
        for c in spec["containers"]:
            resources = c.get("resources") or {}
            assert resources.get("requests", {}).get("memory"), f"{name}/{c['name']} has no memory request"
            assert resources.get("limits", {}).get("memory"), f"{name}/{c['name']} has no memory limit"


def test_node_exporter_shares_neither_the_host_network_nor_its_processes(env):
    ds = next(d for d in of_kind(env, "DaemonSet") if d["metadata"]["name"].endswith("node-exporter"))
    spec = ds["spec"]["template"]["spec"]
    assert ds["metadata"]["namespace"] == "kube-system"
    assert not spec.get("hostNetwork") and not spec.get("hostPID")
    for c in spec["containers"]:
        assert c["securityContext"]["readOnlyRootFilesystem"] is True
        assert all(m.get("readOnly") for m in c.get("volumeMounts", []) if m["name"] in ("proc", "sys", "root"))


def test_nothing_reaches_the_stack_from_the_edge(env):
    for d in of_kind(env, "Ingress"):
        assert "prometheus" not in d["metadata"]["name"] and "alertmanager" not in d["metadata"]["name"]
    for doc in env:
        if doc.get("kind") == "ConfigMap" and "apisix" in doc["metadata"]["name"]:
            text = str(doc.get("data"))
            assert "prometheus-operated" not in text and "alertmanager" not in text


def test_the_targets_admit_this_prometheus_and_node_exporter_admits_only_it(env):
    namespace = one(env, "Prometheus")["metadata"]["namespace"]
    policies = of_kind(env, "NetworkPolicy")
    admitting = [
        p for p in policies
        for rule in p["spec"].get("ingress", [])
        for peer in rule.get("from", [])
        if peer.get("podSelector", {}).get("matchLabels") == {"app.kubernetes.io/name": "prometheus"}
        and peer.get("namespaceSelector", {}).get("matchLabels") == {"kubernetes.io/metadata.name": namespace}
    ]
    names = {p["metadata"]["name"] for p in admitting}
    assert {"node-exporter", "kube-state-metrics", "alertmanager"} <= names
    assert len(names) > 6, f"only {sorted(names)} admit the Prometheus: monitoring.prometheusNamespace is unset"
    node = next(p for p in policies if p["metadata"]["name"] == "node-exporter")
    assert node["metadata"]["namespace"] == "kube-system"
    assert [peer for rule in node["spec"]["ingress"] for peer in rule["from"]] == [
        {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": namespace}},
         "podSelector": {"matchLabels": {"app.kubernetes.io/name": "prometheus"}}}]
    assert node["spec"]["policyTypes"] == ["Egress", "Ingress"] and not node["spec"].get("egress")


def test_prometheus_egress_names_its_namespaces(env):
    server = next(p for p in of_kind(env, "NetworkPolicy") if p["metadata"]["name"] == "prometheus-server")
    namespace = one(env, "Prometheus")["metadata"]["namespace"]
    scrape = server["spec"]["egress"][0]["to"][0]["namespaceSelector"]["matchExpressions"][0]
    assert scrape["operator"] == "In" and {namespace, "kube-system"} <= set(scrape["values"])
    # dev's Prometheus is jc-prod's too (T-6727): it reaches jc-prod's exporters and cert-manager.
    if namespace == "dev":
        assert {"cert-manager", "jc-dev", "jc-prod"} <= set(scrape["values"]), scrape["values"]


def test_the_api_token_is_mounted_only_with_a_reason(env):
    reason = "security.joinedcontext.com/api-access-reason"
    prometheus = one(env, "Prometheus")["spec"]
    assert prometheus["podMetadata"]["annotations"][reason]
    for name, spec in pods(env):
        if spec.get("automountServiceAccountToken", True) and not name.endswith("node-exporter"):
            owner = next(d for d in env if d.get("kind") in ("Deployment", "DaemonSet") and d["metadata"]["name"] == name)
            assert (owner["spec"]["template"]["metadata"].get("annotations") or {}).get(reason), f"{name} mounts a token without a reason"


def test_kube_state_metrics_lists_no_configmap_or_secret(env):
    """It caches every object of a kind it lists; dev's seeds and Helm releases OOMKilled it (T-5434)."""
    roles = [d for d in of_kind(env, "ClusterRole") if "kube-state-metrics" in d["metadata"]["name"]]
    assert roles
    resources = {r for role in roles for rule in role.get("rules", []) for r in rule.get("resources", [])}
    assert not resources & {"configmaps", "secrets"}, resources
