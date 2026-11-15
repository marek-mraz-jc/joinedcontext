"""Tests asserting workload chart renders correctly and complies with security standards."""

import shutil
import subprocess
from pathlib import Path
import pytest
import yaml

CHART = Path(__file__).resolve().parent.parent / "charts/workload"
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")


def render_workload(tmp_path: Path, values: dict | None = None, extra_args: list[str] | None = None) -> list[dict]:
    cmd = ["helm", "template", "test", str(CHART)]
    if values is not None:
        tmp_path.mkdir(parents=True, exist_ok=True)
        val_file = tmp_path / "values.yaml"
        val_file.write_text(yaml.dump(values))
        cmd.extend(["-f", str(val_file)])
    if extra_args:
        cmd.extend(extra_args)
    res = subprocess.run(cmd, capture_output=True, text=True, check=True)
    return [doc for doc in yaml.safe_load_all(res.stdout) if doc]


def find_resource(docs: list[dict], kind: str) -> dict:
    for doc in docs:
        if doc.get("kind") == kind:
            return doc
    raise AssertionError(f"Resource kind={kind} not found in rendered output")


@requires_helm
def test_image_reference_formatting(tmp_path):
    docs_no_digest = render_workload(tmp_path / "no_digest", {"image": {"repository": "myrepo", "tag": "v1"}})
    dep1 = find_resource(docs_no_digest, "Deployment")
    assert dep1["spec"]["template"]["spec"]["containers"][0]["image"] == "myrepo:v1"

    docs_digest = render_workload(
        tmp_path / "digest",
        {"image": {"repository": "myrepo", "tag": "v1", "digest": "sha256:12345"}}
    )
    dep2 = find_resource(docs_digest, "Deployment")
    assert dep2["spec"]["template"]["spec"]["containers"][0]["image"] == "myrepo:v1@sha256:12345"


@requires_helm
def test_rolling_update_strategy_and_pod_security_flags(tmp_path):
    docs = render_workload(tmp_path)
    dep = find_resource(docs, "Deployment")
    sa = find_resource(docs, "ServiceAccount")

    strat = dep["spec"]["strategy"]["rollingUpdate"]
    assert strat["maxUnavailable"] == 0
    assert strat.get("maxSurge") is not None

    pod_spec = dep["spec"]["template"]["spec"]
    assert pod_spec["automountServiceAccountToken"] is False
    assert sa["automountServiceAccountToken"] is False
    assert pod_spec["enableServiceLinks"] is False

    container = pod_spec["containers"][0]
    readiness = container["readinessProbe"]
    liveness = container["livenessProbe"]
    assert readiness is not None and liveness is not None
    assert readiness != liveness


@requires_helm
def test_autoscaling_hpa_and_replicas(tmp_path):
    docs_default = render_workload(tmp_path / "no_hpa", {"replicaCount": 3})
    dep_default = find_resource(docs_default, "Deployment")
    assert dep_default["spec"]["replicas"] == 3
    assert not any(d.get("kind") == "HorizontalPodAutoscaler" for d in docs_default)

    docs_hpa = render_workload(tmp_path / "hpa", {"autoscaling": {"enabled": True, "minReplicas": 2, "maxReplicas": 5}})
    dep_hpa = find_resource(docs_hpa, "Deployment")
    assert "replicas" not in dep_hpa["spec"]
    hpa = find_resource(docs_hpa, "HorizontalPodAutoscaler")
    assert hpa["spec"]["minReplicas"] == 2


@requires_helm
@pytest.mark.parametrize("values", [{"replicaCount": 2}, {"autoscaling": {"enabled": True, "minReplicas": 2}}])
def test_replicas_spread_over_nodes_so_one_drain_never_takes_them_all(tmp_path, values):
    pod = find_resource(render_workload(tmp_path, values), "Deployment")["spec"]["template"]["spec"]
    [spread] = pod["topologySpreadConstraints"]
    assert spread["topologyKey"] == "kubernetes.io/hostname"
    assert spread["maxSkew"] == 1
    # Soft: a one-node cluster (dev, today's prod) still schedules every replica.
    assert spread["whenUnsatisfiable"] == "ScheduleAnyway"
    assert spread["labelSelector"]["matchLabels"] == pod_selector(tmp_path)


def pod_selector(tmp_path: Path) -> dict:
    return find_resource(render_workload(tmp_path / "sel"), "Deployment")["spec"]["selector"]["matchLabels"]


@requires_helm
def test_a_single_replica_carries_no_spread(tmp_path):
    pod = find_resource(render_workload(tmp_path), "Deployment")["spec"]["template"]["spec"]
    assert "topologySpreadConstraints" not in pod


@requires_helm
def test_a_budget_that_leaves_room_for_one_eviction_renders(tmp_path):
    docs = render_workload(tmp_path, {"replicaCount": 2, "podDisruptionBudget": {"enabled": True, "minAvailable": 1}})
    assert find_resource(docs, "PodDisruptionBudget")["spec"]["minAvailable"] == 1


@requires_helm
@pytest.mark.parametrize("values", [
    {"replicaCount": 1, "podDisruptionBudget": {"enabled": True, "minAvailable": 1}},
    {"autoscaling": {"enabled": True, "minReplicas": 2}, "podDisruptionBudget": {"enabled": True, "minAvailable": 2}},
])
def test_a_budget_that_blocks_every_drain_is_refused(tmp_path, values):
    with pytest.raises(subprocess.CalledProcessError) as err:
        render_workload(tmp_path, values)
    assert "no node can ever be drained" in err.value.stderr


@requires_helm
def test_env_raw_value_and_env_secret(tmp_path):
    values = {
        "env": {"ANTARES_DATABASE_URL": "postgresql://antares:$(PGPASSWORD)@localhost:5432/antares"},
        "envSecret": {"PGPASSWORD": {"secret": "db-antares", "key": "password"}},
    }
    docs = render_workload(tmp_path, values)
    dep = find_resource(docs, "Deployment")
    env_vars = dep["spec"]["template"]["spec"]["containers"][0]["env"]

    url_env = next(e for e in env_vars if e["name"] == "ANTARES_DATABASE_URL")
    assert url_env["value"] == "postgresql://antares:$(PGPASSWORD)@localhost:5432/antares"

    secret_env = next(e for e in env_vars if e["name"] == "PGPASSWORD")
    assert secret_env["valueFrom"]["secretKeyRef"] == {"name": "db-antares", "key": "password"}


@requires_helm
def test_an_optional_env_secret_key_may_be_absent(tmp_path):
    """T-2842: a key only a rotation writes must not hold the pod in CreateContainerConfigError."""
    values = {"envSecret": {"PREV": {"secret": "s", "key": "previous", "optional": True}}}
    env_vars = find_resource(render_workload(tmp_path, values), "Deployment")["spec"]["template"]["spec"]["containers"][0]["env"]
    assert env_vars[0]["valueFrom"]["secretKeyRef"] == {"name": "s", "key": "previous", "optional": True}


@requires_helm
def test_configmap_empty_data_renders_and_annotations(tmp_path):
    docs = render_workload(tmp_path, {"configMap": {"enabled": True, "mountPath": "/streams", "data": {}}})
    cm = find_resource(docs, "ConfigMap")
    assert cm["data"] == {}

    dep = find_resource(docs, "Deployment")
    annotations = dep["spec"]["template"]["metadata"]["annotations"]
    assert "checksum/config" in annotations


@requires_helm
def test_a_workload_with_nothing_to_serve_is_ready_by_command_and_never_restarted_by_a_probe(tmp_path):
    """The forge's runner (components/gitea-runner) serves no HTTP. Kyverno's validate-probes
    wants a probe on every container; a liveness restart would lose the registration token the
    runner read once and deleted, so readiness comes from a command and liveness stays off."""
    docs = render_workload(tmp_path, {
        "image": {"repository": "busybox", "tag": "1"},
        "service": {"enabled": False},
        "probes": {"enabled": False, "readinessExec": ["test", "-s", "/tmp/runner/.runner"]},
    })
    container = find_resource(docs, "Deployment")["spec"]["template"]["spec"]["containers"][0]
    assert container["readinessProbe"]["exec"]["command"] == ["test", "-s", "/tmp/runner/.runner"]
    assert "livenessProbe" not in container
    assert "httpGet" not in container["readinessProbe"]

    bare = render_workload(tmp_path, {"image": {"repository": "busybox", "tag": "1"},
                                      "service": {"enabled": False}, "probes": {"enabled": False}})
    plain = find_resource(bare, "Deployment")["spec"]["template"]["spec"]["containers"][0]
    assert "readinessProbe" not in plain and "livenessProbe" not in plain, "no command, no probe"


@requires_helm
def test_a_stopping_pod_keeps_serving_until_the_edge_and_the_mesh_let_it_go(tmp_path):
    """T-3008, OPS-27: every roll answered ~2 s of 502s in 0.1 s. The kubelet stops the old pod at
    once while APISIX and Linkerd still send to it, and the meshed proxy refuses new connections
    from the same instant. The app pauses first (the kubelet's own sleep: the images are
    distroless), the proxy outlives the pause and the drain, and the grace period outlives both."""
    dep = find_resource(render_workload(tmp_path / "meshed", {"serviceMesh": {"enabled": True}}), "Deployment")
    pod = dep["spec"]["template"]
    pause = pod["spec"]["containers"][0]["lifecycle"]["preStop"]["sleep"]["seconds"]
    assert pause >= 5
    wait = int(pod["metadata"]["annotations"]["config.alpha.linkerd.io/proxy-wait-before-exit-seconds"])
    assert wait > pause
    assert pod["spec"]["terminationGracePeriodSeconds"] > wait

    # Without the mesh there is no proxy to hold; the app still pauses.
    plain = find_resource(render_workload(tmp_path / "plain", {"serviceMesh": {"enabled": False}}), "Deployment")
    template = plain["spec"]["template"]
    assert "config.alpha.linkerd.io/proxy-wait-before-exit-seconds" not in (template["metadata"].get("annotations") or {})
    assert template["spec"]["containers"][0]["lifecycle"]["preStop"]["sleep"]["seconds"] == pause

    # A workload that opts out renders neither.
    off = find_resource(
        render_workload(tmp_path / "off", {"preStopSleepSeconds": 0, "serviceMesh": {"enabled": True}}), "Deployment"
    )
    assert "lifecycle" not in off["spec"]["template"]["spec"]["containers"][0]
    assert "config.alpha.linkerd.io/proxy-wait-before-exit-seconds" not in off["spec"]["template"]["metadata"]["annotations"]


@requires_helm
def test_a_grace_period_shorter_than_the_pause_and_drain_is_refused(tmp_path):
    """A kubelet that kills the pod before its proxy's wait ends cuts the drain it was for."""
    with pytest.raises(subprocess.CalledProcessError) as refused:
        render_workload(tmp_path, {"terminationGracePeriodSeconds": 10, "serviceMesh": {"enabled": True}})
    assert "terminationGracePeriodSeconds (10) must be longer" in refused.value.stderr


@requires_helm
@pytest.mark.parametrize(
    "limit, expected",
    [("1536Mi", 1536 * 2**20 * 85 // 100), ("2Gi", 2 * 2**30 * 85 // 100), ("1G", 10**9 * 85 // 100), ("1000000", 850000)],
)
def test_gomemlimit_follows_the_memory_limit(tmp_path, limit, expected):
    """T-3537: a Go workload's soft limit is a share of whatever limit the environment sets."""
    values = {"goMemLimitPercent": 85, "resources": {"limits": {"memory": limit}}}
    env = find_resource(render_workload(tmp_path, values), "Deployment")["spec"]["template"]["spec"]["containers"][0]["env"]
    assert {"name": "GOMEMLIMIT", "value": str(expected)} in env


@requires_helm
def test_gomemlimit_is_off_by_default_and_refuses_what_it_cannot_read(tmp_path):
    container = find_resource(render_workload(tmp_path / "default"), "Deployment")["spec"]["template"]["spec"]["containers"][0]
    assert all(e["name"] != "GOMEMLIMIT" for e in container.get("env", []))
    for name, values, reason in [
        ("fraction", {"goMemLimitPercent": 85, "resources": {"limits": {"memory": "1.5Gi"}}}, "not a whole number"),
        ("percent", {"goMemLimitPercent": 120}, "not between 1 and 100"),
    ]:
        with pytest.raises(subprocess.CalledProcessError) as refused:
            render_workload(tmp_path / name, values)
        assert reason in refused.value.stderr, name


@requires_helm
def test_recreate_stops_the_old_pod_without_a_rolling_block(tmp_path):
    """T-5311: a single-writer workload opts into Recreate; Kubernetes refuses rollingUpdate beside it."""
    dep = find_resource(render_workload(tmp_path, {"updateStrategy": "Recreate"}), "Deployment")
    assert dep["spec"]["strategy"] == {"type": "Recreate"}


@requires_helm
def test_an_unknown_update_strategy_fails_the_render(tmp_path):
    with pytest.raises(subprocess.CalledProcessError):
        render_workload(tmp_path, {"updateStrategy": "OnDelete"})


def test_every_value_a_template_reads_is_in_the_strict_schema():
    """T-6218 (R2DEP1 F-009, W-008): the schema refuses any key it does not list, so a template
    branch reading an unlisted key can never run and setting the key fails the render."""
    import json
    import re
    schema = json.loads((CHART / "values.schema.json").read_text())
    missing = set()
    for template in (CHART / "templates").glob("*"):
        for path in re.findall(r"\.Values((?:\.[A-Za-z_][A-Za-z0-9_]*)+)", template.read_text()):
            node = schema
            for key in path.strip(".").split("."):
                properties = node.get("properties")
                if properties is None:
                    break
                if key not in properties:
                    missing.add(f"{template.name}: .Values{path}")
                    break
                node = properties[key]
    assert not missing, sorted(missing)


@requires_helm
def test_the_startup_probe_asks_what_liveness_asks(tmp_path):
    """T-6218 (R2DEP1 F-010): a workload whose liveness is a command is held by a startup probe
    running that command, not an HTTP GET of a path it may not serve."""
    command = ["/bin/sh", "-c", "test -f /tmp/alive"]
    docs = render_workload(tmp_path, {"probes": {"enabled": True, "liveness": {"exec": command},
                                                 "startup": {"failureThreshold": 20}}})
    container = find_resource(docs, "Deployment")["spec"]["template"]["spec"]["containers"][0]
    assert container["startupProbe"]["exec"]["command"] == command
    assert "httpGet" not in container["startupProbe"]
    plain = render_workload(tmp_path, {"probes": {"enabled": True, "startup": {"failureThreshold": 20}}})
    container = find_resource(plain, "Deployment")["spec"]["template"]["spec"]["containers"][0]
    assert container["startupProbe"]["httpGet"]["path"] == "/"
