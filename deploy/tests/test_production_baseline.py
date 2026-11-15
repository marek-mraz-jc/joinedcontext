"""The production baseline: what every workload must carry before an instance is called
production, and the scrape configuration that makes it observable (T-0038, T-0040, T-0041 —
OPS-04, OPS-05, OPS-06, OPS-07, OPS-08, OPS-16, TS-22).

The three single-replica deployments are named here on purpose. Each is a documented,
deliberate exception with a reason in its own production values file, and naming them means a
FOURTH one cannot appear without someone editing this list and saying why.
"""

import re
import pytest
import yaml

WORKLOAD_KINDS = ("Deployment", "StatefulSet")

# Multi-replica workloads that deliberately carry no PodDisruptionBudget, and why.
NO_BUDGET = {
    "postgres-operator-cloudnative-pg": (
        "the operator is leader-elected and is not in the data path: a Postgres cluster keeps "
        "serving while it is down, so a drain that takes both replicas delays reconciliation "
        "rather than causing an outage. The upstream chart templates no PDB for itself"
    ),
    "gitea-runner": (
        "no traffic reaches it and nothing waits on it: a runner between jobs is safe to evict, "
        "a build it was running fails and is re-run from the forge, and queued builds wait in "
        "the forge while no runner is up (components/gitea-runner production values, T-2608)"
    ),
}

# Workloads that mount a path from the node, and why each one has to. A hostPath is host
# access: it survives the pod, it is not namespaced, and PSS baseline forbids it outright. So a
# new one may not appear without an entry here and a colocated PolicyException, which is what
# the kyverno lane checks. Naming them means the fourth cannot arrive quietly.
HOST_PATHS = {
    "audit-logging-collector-vector": (
        "one Vector agent per node reads that node's own container logs, and a node's logs "
        "live on the node; the mounts are read-only and narrowed to what `kubernetes_logs` "
        "needs, with the reason in components/audit-logging/policy-exceptions.yaml"
    ),
}

# Workloads that stay at one replica in production, and the reason each one does.
SINGLE_REPLICA = {
    "context-broker": "ANTARES_BUS=local allows one broker process per database; raising it needs a NATS bus",
    "pipeline-runner": "scaling is per-project runner pools (PL-12), not replicas of one pool",
    "gitea": "one ReadWriteOnce volume holds the bare repositories, so the forge fails over rather than scaling out",
    "artifact-store": "one process over one ReadWriteOnce volume; a second replica needs an erasure-coded pool across nodes, not a replica count",
}


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


def workloads(docs):
    return [d for d in docs if d.get("kind") in WORKLOAD_KINDS]


def of_kind(docs, kind):
    return [d for d in docs if d.get("kind") == kind]


def test_every_container_declares_requests_and_limits(production):
    # OPS-05: a container without limits can starve every other workload on its node, and the
    # restricted Pod Security Standard has nothing to say about it.
    missing = []
    for workload in workloads(production):
        spec = workload["spec"]["template"]["spec"]
        for container in spec.get("containers", []) + spec.get("initContainers", []):
            resources = container.get("resources") or {}
            if not (resources.get("requests") and resources.get("limits")):
                missing.append(f"{workload['metadata']['name']}/{container['name']}")
    assert missing == [], f"containers without both requests and limits: {missing}"


def test_stateless_services_run_at_least_two_replicas(production):
    # OPS-04. A workload allowed to run alone is listed above with its reason; anything else
    # having one replica is a production instance with a single point of failure.
    single = {}
    for workload in workloads(production):
        replicas = workload["spec"].get("replicas")
        if replicas is not None and replicas < 2:
            single[workload["metadata"]["name"]] = replicas
    unexplained = sorted(set(single) - set(SINGLE_REPLICA))
    assert unexplained == [], (
        f"{unexplained} run one replica in production with no recorded reason. "
        "Give the workload two replicas, or add it to SINGLE_REPLICA with why it cannot have them."
    )


def test_every_multi_replica_workload_has_a_disruption_budget(production):
    # OPS-08: without a budget a node drain can take every replica of a service at once, and a
    # cluster upgrade becomes an outage.
    budgets = {
        tuple(sorted((pdb["spec"].get("selector") or {}).get("matchLabels", {}).items()))
        for pdb in of_kind(production, "PodDisruptionBudget")
    }
    uncovered = []
    for workload in workloads(production):
        replicas = workload["spec"].get("replicas")
        if replicas is None or replicas < 2:
            continue
        labels = workload["spec"]["selector"]["matchLabels"]
        covered = any(set(budget).issubset(set(labels.items())) for budget in budgets)
        if not covered and workload["metadata"]["name"] not in NO_BUDGET:
            uncovered.append(workload["metadata"]["name"])
    assert uncovered == [], (
        f"multi-replica workloads without a PodDisruptionBudget: {uncovered}. "
        "Give the component a budget, or add it to NO_BUDGET with why it does not need one."
    )


def test_disruption_budgets_keep_at_least_one_pod(production):
    for pdb in of_kind(production, "PodDisruptionBudget"):
        assert pdb["spec"].get("minAvailable") == 1 or pdb["spec"].get("maxUnavailable") is not None, (
            f"{pdb['metadata']['name']} expresses no budget"
        )


def test_rollouts_never_dip_below_the_replica_count(production):
    # OPS-07: maxUnavailable 0 is the half that keeps the service up during a rollout; the
    # surge is a share of the replicas rather than a fixed pod.
    for workload in of_kind(production, "Deployment"):
        strategy = workload["spec"].get("strategy", {})
        if strategy.get("type") == "Recreate":
            assert workload["metadata"]["name"] in SINGLE_REPLICA, (
                f"{workload['metadata']['name']} recreates its pods, which is an outage for a "
                "workload that is supposed to be highly available"
            )
            continue
        rolling = strategy.get("rollingUpdate", {})
        assert rolling.get("maxUnavailable") == 0, (
            f"{workload['metadata']['name']} may go below its replica count during a rollout"
        )


def test_the_scrape_targets_are_the_components_that_export_metrics(production):
    # OPS-16: every core component is scraped, and each exactly once. A second monitor on the
    # same target doubles every counter, which is worse than none.
    monitors = of_kind(production, "ServiceMonitor") + of_kind(production, "PodMonitor")
    names = [m["metadata"]["name"] for m in monitors]
    assert len(names) == len(set(names)), f"a target is scraped twice: {sorted(names)}"
    for expected in ("context-gateway", "context-broker", "portal", "pipeline-runner"):
        assert expected in names, f"{expected} exports metrics but nothing scrapes it"
    assert "apisix" in names, "the edge must be scraped (its own chart renders the monitor)"


def test_every_monitor_scrapes_every_fifteen_seconds_by_name(production):
    for monitor in of_kind(production, "ServiceMonitor") + of_kind(production, "PodMonitor"):
        if monitor["metadata"]["name"].startswith("keycloak"):
            continue  # the upstream keycloak chart sets its own interval
        endpoints = monitor["spec"].get("endpoints") or monitor["spec"].get("podMetricsEndpoints")
        for endpoint in endpoints:
            assert endpoint.get("interval") == "15s", f"{monitor['metadata']['name']} scrapes at {endpoint.get('interval')}"
            # A port named, not numbered: a renamed port must break the monitor loudly instead
            # of scraping whatever now sits on that number.
            assert endpoint.get("port") or endpoint.get("targetPort"), (
                f"{monitor['metadata']['name']} names no port"
            )


def test_a_monitor_scrapes_the_path_its_target_actually_serves(production):
    # A monitor on the wrong path reports the target down for as long as nobody looks, which
    # is indistinguishable from a target that is down (OPS-16). Antares mounts its Prometheus
    # text inside the `q` surface; everything else answers on the chart's `/metrics` default.
    served = {"context-broker": "/q/metrics"}
    for monitor in of_kind(production, "ServiceMonitor"):
        expected = served.get(monitor["metadata"]["name"])
        if not expected:
            continue
        paths = [endpoint.get("path") for endpoint in monitor["spec"]["endpoints"]]
        assert paths == [expected], f"{monitor['metadata']['name']} scrapes {paths}"


def test_the_broker_has_its_metrics_recorder_switched_on(production):
    # Antares builds no recorder unless ANTARES_TELEMETRY is set, and then /q/metrics answers
    # 404 whatever the monitor asks for.
    broker = [
        workload
        for workload in of_kind(production, "StatefulSet") + of_kind(production, "Deployment")
        if workload["metadata"]["name"].startswith("context-broker")
    ]
    assert broker, "the production environment renders no context-broker workload"
    for workload in broker:
        for container in workload["spec"]["template"]["spec"]["containers"]:
            names = {variable["name"] for variable in container.get("env", [])}
            assert "ANTARES_TELEMETRY" in names, (
                f"{workload['metadata']['name']}/{container['name']} exports no metrics"
            )


def test_a_monitor_only_selects_its_own_namespace(production):
    # A monitor that matched every namespace would scrape a second instance on the same cluster
    # and mix two cities' numbers into one series.
    for monitor in of_kind(production, "ServiceMonitor") + of_kind(production, "PodMonitor"):
        selector = monitor["spec"].get("namespaceSelector", {})
        if not selector:
            continue
        assert selector.get("matchNames") == [monitor["metadata"]["namespace"]], (
            f"{monitor['metadata']['name']} scrapes beyond its own namespace: {selector}"
        )


def test_the_edge_alerts_are_the_two_the_runbook_names(production):
    rules = of_kind(production, "PrometheusRule")
    assert rules, "OPS-16: the edge alerting rules must be part of a production render"
    alerts = {
        rule["alert"]
        for prometheus_rule in rules
        for group in prometheus_rule["spec"]["groups"]
        for rule in group["rules"]
        if "alert" in rule
    }
    assert {"APISIXConfigReloadFailed", "APISIXHigh5xxRate"} <= alerts, (
        f"docs Deployment/10 section 7 names two alerts; the render has {sorted(alerts)}"
    )


def test_a_broker_whose_pool_runs_dry_pages_somebody(production):
    """T-4298: the series is the broker's own (antares-sql store/any.rs `db`), so the rule fires
    on the first refused request and holds for two minutes."""
    rules = {
        rule["alert"]: rule
        for prometheus_rule in of_kind(production, "PrometheusRule")
        for group in prometheus_rule["spec"]["groups"]
        for rule in group["rules"]
        if "alert" in rule
    }
    dry = rules["ContextBrokerPoolExhausted"]
    assert dry["expr"] == "sum(increase(antares_pg_pool_timeouts_total[5m])) > 0"
    assert (dry["for"], dry["labels"]["severity"]) == ("2m", "critical")


def test_the_assistant_alerts_read_the_portals_own_series(production):
    """T-2771, AG-72: a slow assistant and a run of failures page somebody, and each rule reads a
    series the Portal exports, so neither can fire never the way a misspelt name would."""
    rules = {
        rule["alert"]: rule
        for prometheus_rule in of_kind(production, "PrometheusRule")
        for group in prometheus_rule["spec"]["groups"]
        for rule in group["rules"]
        if "alert" in rule
    }
    slow = rules["AssistantSlowAnswers"]
    assert "histogram_quantile(0.95" in slow["expr"]
    assert "jc_agent_answer_duration_seconds_bucket" in slow["expr"] and slow["expr"].endswith("> 10")
    assert slow["for"] == "15m"
    failing = rules["AssistantRunsFailing"]
    assert 'jc_agent_runs_finished_total{status=~"failed|expired"}' in failing["expr"]
    assert failing["expr"].endswith("> 0.2")
    # T-3065, AG-96: a dead or empty model key pages somebody, from the gauges the Portal keeps.
    dead = rules["ModelKeyInvalid"]
    assert dead["expr"] == "max(jc_model_key_valid) == 0"
    assert dead["labels"]["severity"] == "critical"
    low = rules["ModelKeyCreditLow"]
    assert "jc_model_key_remaining / jc_model_key_limit" in low["expr"] and low["expr"].endswith("< 0.2")
    # T-4892: a snapshot pipeline whose orphans outlive its delete rule, or whose source lists
    # records the space never took, tells somebody, per project and pipeline.
    stuck = rules["PipelineOrphansStuck"]
    assert stuck["expr"] == "max by (project, pipeline) (jc_pipeline_orphans) > 0"
    assert stuck["for"] == "72h"
    missing = rules["PipelineRecordsMissing"]
    assert missing["expr"] == "max by (project, pipeline) (jc_pipeline_missing) > 10"
    assert "{{ $labels.pipeline }}" in missing["annotations"]["summary"]
    # T-5020: a running pipeline whose newest entity is older than its freshness target tells
    # somebody, from the gauge the Portal's quality run sets.
    stale = rules["PipelineDataStale"]
    assert stale["expr"] == "max by (project, pipeline) (jc_pipeline_stale) == 1"
    assert stale["labels"]["severity"] == "warning"
    assert "{{ $labels.pipeline }}" in stale["annotations"]["summary"]


def test_metrics_stay_inside_the_cluster(production):
    # The scrape ports are never published: an Ingress or an APISIX route to a metrics path
    # would put the platform's internals on the public host.
    metrics_paths = ("/metrics", "/apisix/prometheus/metrics")
    for ingress in of_kind(production, "Ingress"):
        for rule in ingress["spec"].get("rules", []):
            for path in rule.get("http", {}).get("paths", []):
                assert not any(path.get("path", "").startswith(p) for p in metrics_paths), (
                    f"{ingress['metadata']['name']} publishes a metrics path"
                )
    for config_map in of_kind(production, "ConfigMap"):
        routes = (config_map.get("data") or {}).get("apisix.yaml")
        if not routes:
            continue
        assert "/apisix/prometheus/metrics" not in routes, (
            "the edge must not route to its own metrics endpoint"
        )


def test_the_component_renders_nothing_without_a_prometheus(rendered):
    # Scrape configuration with no Prometheus watching is dead configuration; the local profile
    # leaves global.metrics.enabled off and must therefore carry no monitor at all.
    local = rendered("local")
    assert of_kind(local, "ServiceMonitor") == []
    assert of_kind(local, "PrometheusRule") == []


def test_only_the_named_workloads_reach_into_the_node(production):
    """A hostPath that nobody wrote down is host access nobody agreed to (OPS-04).

    The Kyverno lane refuses one outright, so this test exists to fail first and locally, with
    the name of the workload and a pointer at the two files that have to change together: the
    entry above and the component's own PolicyException.
    """
    mounting = {
        document["metadata"]["name"]
        for document in production
        if document.get("kind") in ("Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob")
        for volume in _pod_spec(document).get("volumes") or []
        if "hostPath" in volume
    }
    assert mounting <= set(HOST_PATHS), (
        f"{sorted(mounting - set(HOST_PATHS))} mount a path from the node with no entry in "
        "HOST_PATHS. Add one with the reason, and a PolicyException colocated with the "
        "component, or stop mounting the host."
    )


def test_the_audit_collector_mounts_no_more_of_the_node_than_it_reads(production):
    """The chart also offers /proc and /sys for its `host_metrics` source, which this
    collector does not have. A chart bump that restores the defaults would hand the agent
    host access nothing reads, and would do it silently."""
    collector = [
        document
        for document in production
        if document.get("kind") == "DaemonSet"
        and document["metadata"]["name"] == "audit-logging-collector-vector"
    ]
    assert len(collector) == 1, "the audit collector renders in production"
    paths = {
        volume["hostPath"]["path"].rstrip("/")
        for volume in _pod_spec(collector[0]).get("volumes") or []
        if "hostPath" in volume
    }
    assert paths <= {"/var/log", "/var/lib", "/var/lib/vector"}, (
        f"the collector mounts {sorted(paths)}; /proc and /sys belong to the chart's "
        "host_metrics default and this collector has no such source"
    )
    for volume_mount in _pod_spec(collector[0])["containers"][0].get("volumeMounts") or []:
        if volume_mount["name"] in ("var-log", "var-lib"):
            assert volume_mount.get("readOnly") is True, (
                f"{volume_mount['name']} is the node's own log tree and is only ever read"
            )


def _pod_spec(document: dict) -> dict:
    """The pod spec of any workload kind, which each one buries at a different depth."""
    spec = document.get("spec") or {}
    if document.get("kind") == "CronJob":
        spec = (spec.get("jobTemplate") or {}).get("spec") or {}
    return (spec.get("template") or {}).get("spec") or {}


# The labels APISIX's prometheus plugin puts on `apisix_http_status`. A selector on any other
# label matches no series, so the alert built on it can never fire; the dashboards already read
# `code` (components/grafana/charts/dashboards/files/edge-apisix.json).
APISIX_HTTP_STATUS_LABELS = {"code", "route", "matched_uri", "matched_host", "service", "consumer", "node"}


def test_an_edge_alert_selects_only_labels_apisix_emits(production):
    """T-1715: `APISIXHigh5xxRate` once selected `status=~"5.."`, a label APISIX never emits, so
    a node taken down by one caller raised no alert at all."""
    offenders = []
    for prometheus_rule in of_kind(production, "PrometheusRule"):
        for group in prometheus_rule["spec"]["groups"]:
            for rule in group["rules"]:
                for selector in re.findall(r"apisix_http_status\{([^}]*)\}", rule.get("expr", "")):
                    for label in re.findall(r"(\w+)\s*(?:=~|!~|!=|=)", selector):
                        if label not in APISIX_HTTP_STATUS_LABELS:
                            offenders.append(f"{rule.get('alert')}: {label}")
    assert not offenders, "alerts that can never fire:\n" + "\n".join(offenders)
