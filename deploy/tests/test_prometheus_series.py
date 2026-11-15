"""The series diet of the prometheus component (T-5925) drops no metric an alert rule, a recording
rule, a Grafana dashboard or scripts/mem-sample.py reads.

Three places drop series: `drop` metric relabelings on __name__ in a ServiceMonitor or PodMonitor,
the kube-state-metrics `--metric-denylist`, and the kube-state-metrics `--resources` list (a kind
it does not list yields no kube_<kind>_* family). A relabeling that also matches on another label
(`[__name__, le]`) drops only some series of a metric and is not counted here."""

import json
import re

import pytest
import yaml
from conftest import PROJECT_ROOT

DASHBOARDS = PROJECT_ROOT / "components/grafana/charts/dashboards/files"
SCRIPTS = [PROJECT_ROOT / "scripts/mem-sample.py"]
NAME = re.compile(r"[a-zA-Z_:][a-zA-Z0-9_:]*")
# Dropped on purpose although a rule names it. The chart's own cAdvisor default drops it, and its
# only reader is the chart's node_namespace_pod_container:container_memory_swap recording rule,
# whose output no rule or dashboard reads (the node has no swap).
UNREAD = {"container_memory_swap"}


def exprs(node):
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "expr" and isinstance(value, str):
                yield value
            else:
                yield from exprs(value)
    elif isinstance(node, list):
        for item in node:
            yield from exprs(item)


def read_metrics(docs):
    """Every identifier of every query: metric names plus label names and functions, which only
    makes the check stricter (a label name never fully matches a drop pattern)."""
    texts = []
    for d in docs:
        if d.get("kind") == "PrometheusRule":
            texts += [r["expr"] for g in d["spec"]["groups"] for r in g["rules"]]
    for path in sorted(DASHBOARDS.glob("*.json")):
        texts += list(exprs(json.loads(path.read_text())))
    texts += [p.read_text() for p in SCRIPTS]
    return {n for text in texts for n in NAME.findall(text)}


def name_drops(docs):
    """(where, full-match pattern) of every relabeling that drops a metric by its name alone."""
    for d in docs:
        if d.get("kind") not in ("ServiceMonitor", "PodMonitor"):
            continue
        spec = d["spec"]
        for e in spec.get("endpoints", []) + spec.get("podMetricsEndpoints", []):
            for r in e.get("metricRelabelings", []):
                if r.get("action", "replace") == "drop" and r.get("sourceLabels") == ["__name__"]:
                    yield f"{d['metadata']['name']} {e.get('path', '/metrics')}", re.compile(r["regex"])


def ksm_args(docs):
    (deploy,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("kube-state-metrics")]
    (container,) = deploy["spec"]["template"]["spec"]["containers"]
    return dict(a.removeprefix("--").split("=", 1) for a in container["args"] if "=" in a)


def dropped(metric, drops, denylist, resources):
    for where, pattern in drops:
        if pattern.fullmatch(metric):
            return f"dropped by {where} ({pattern.pattern})"
    for pattern in denylist:
        if pattern.search(metric):
            return f"denied by kube-state-metrics ({pattern.pattern})"
    if metric.startswith("kube_") and not metric.startswith("kube_state_metrics_"):
        kind = metric.split("_")[1]
        if not {kind + "s", kind + "es"} & resources:
            return f"kube-state-metrics lists no {kind}"
    return None


def problems(docs):
    args = ksm_args(docs)
    denylist = [re.compile(p) for p in args.get("metric-denylist", "").split(",") if p]
    resources = set(args["resources"].split(","))
    drops = list(name_drops(docs))
    out = []
    for metric in sorted(read_metrics(docs) - UNREAD):
        why = dropped(metric, drops, denylist, resources)
        if why:
            out.append(f"{metric}: {why}")
    return out


@pytest.fixture(scope="module", params=["dev", "smartcity"])
def env(request, rendered):
    return rendered(request.param)


def test_no_rule_dashboard_or_script_reads_a_dropped_metric(env):
    assert read_metrics(env) >= {"kubelet_volume_stats_used_bytes", "container_memory_working_set_bytes", "kube_pod_info"}
    assert problems(env) == []


def test_the_diet_is_rendered(env):
    """The drops this check guards are the ones in the values: an empty set would pass anything."""
    drops = [p.pattern for _, p in name_drops(env)]
    assert any("apiserver" in p and "etcd" in p for p in drops), drops
    assert any("blkio_device_usage_total" in p for p in drops), drops
    args = ksm_args(env)
    assert "kube_pod_init_container_" in args["metric-denylist"]
    assert "services" not in args["resources"].split(",")


def test_a_drop_that_hits_a_read_metric_goes_red(env):
    docs = yaml.safe_load(yaml.safe_dump(env))
    monitor = next(d for d in docs if d.get("kind") == "ServiceMonitor" and d["metadata"]["name"] == "prometheus-kubelet")
    monitor["spec"]["endpoints"][0]["metricRelabelings"].append(
        {"action": "drop", "sourceLabels": ["__name__"], "regex": "kubelet_volume_stats_.+"})
    deploy = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("kube-state-metrics"))
    args = deploy["spec"]["template"]["spec"]["containers"][0]["args"]
    deploy["spec"]["template"]["spec"]["containers"][0]["args"] = [
        a.replace("cronjobs,", "") if a.startswith("--resources=") else a for a in args]
    found = problems(docs)
    assert any(p.startswith("kubelet_volume_stats_used_bytes: dropped by prometheus-kubelet") for p in found), found
    assert "kube_cronjob_status_last_successful_time: kube-state-metrics lists no cronjob" in found, found
