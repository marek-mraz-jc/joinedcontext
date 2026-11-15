"""T-0052: the audit trail of the gateway, Keycloak and the forge reaches append-only storage.

The collector configuration is read out of the rendered ConfigMap and handed to the pinned
Vector image, so what `vector validate` and `vector test` judge is the text that reaches the
cluster (OPS-42, R42, AG-19).
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

# Writes `deployment/environments/testing` in its own copy of the tree (`own_tree`), so it
# shares nothing with another module. Its own xdist group keeps the module on one worker, so its
# module-scoped renders happen once rather than once per worker (T-2895).
pytestmark = pytest.mark.xdist_group("audit-logging")

IMAGE = "docker.io/timberio/vector:0.58.0-distroless-libc"
requires_docker = pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


def one(docs: list[dict], kind: str, name: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


@pytest.fixture(scope="module")
def config(production) -> dict:
    raw = one(production, "ConfigMap", "audit-logging-collector-vector")["data"]["vector.yaml"]
    return yaml.safe_load(raw)


def run_vector(tmp_path: Path, config: dict, *args: str) -> subprocess.CompletedProcess:
    (tmp_path / "vector.yaml").write_text(yaml.safe_dump(config, default_flow_style=False))
    return subprocess.run(
        ["docker", "run", "--rm", "-e", "ACCESS_KEY_ID=stub", "-e", "SECRET_ACCESS_KEY=stub",
         "-v", f"{tmp_path}:/cfg:ro", "--entrypoint", "vector", IMAGE, *args, "/cfg/vector.yaml"],
        capture_output=True, text=True,
    )


@requires_docker
def test_the_rendered_configuration_is_one_vector_accepts(config, tmp_path):
    """A configuration Vector rejects is a collector in CrashLoopBackOff and an audit trail
    that never starts."""
    result = run_vector(tmp_path, config, "validate", "--no-environment")
    assert result.returncode == 0, result.stdout + result.stderr


AUDIT_RECORD = json.dumps({
    "timestamp": "2026-08-20T10:15:30.123Z",
    "level": "INFO",
    "component": "context-gateway",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "action": "queryEntity",
    "verdict": "DENY",
    "matched_policy": "urn:ngsi-ld:Policy:hel.fi:parking:parking-read-v1",
})

POD_FIELDS = {
    "kubernetes.pod_name": "context-gateway-6d4f",
    "kubernetes.pod_namespace": "jc",
    "kubernetes.pod_node_name": "node-1",
    "kubernetes.container_name": "gateway",
    'kubernetes.pod_labels."app.kubernetes.io/name"': "context-gateway-gateway",
}

UNIT_TESTS = [
    {
        "name": "an RFC 3339 record keeps its own instant and its own fields",
        "inputs": [{"insert_at": "audit_record", "type": "log",
                    "log_fields": {"message": AUDIT_RECORD, **POD_FIELDS}}],
        "outputs": [{"extract_from": "audit_record", "conditions": [{"type": "vrl", "source": """
            assert_eq!(.jc_structured, true)
            assert_eq!(.jc_timestamp_source, "record")
            assert_eq!(.verdict, "DENY")
            assert_eq!(.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736")
            assert_eq!(format_timestamp!(.timestamp, "%Y-%m-%dT%H:%M:%SZ"), "2026-08-20T10:15:30Z")
            assert_eq!(.jc_component, "context-gateway-gateway")
            assert_eq!(.jc_pod, "context-gateway-6d4f")
            assert_eq!(.jc_node, "node-1")
        """}]}],
    },
    {
        "name": "a record whose timestamp does not parse keeps its ingest time and says so",
        "inputs": [{"insert_at": "audit_record", "type": "log",
                    "log_fields": {"message": json.dumps({"timestamp": "last tuesday", "level": "WARN"}),
                                   **POD_FIELDS}}],
        "outputs": [{"extract_from": "audit_record", "conditions": [{"type": "vrl", "source": """
            assert_eq!(.jc_timestamp_source, "ingest")
            assert_eq!(.level, "WARN")
            assert!(is_timestamp(.timestamp))
        """}]}],
    },
    {
        "name": "a line that is not JSON is still shipped",
        "inputs": [{"insert_at": "audit_record", "type": "log",
                    "log_fields": {"message": "panicked at src/main.rs:41", **POD_FIELDS}}],
        "outputs": [{"extract_from": "audit_record", "conditions": [{"type": "vrl", "source": """
            assert_eq!(.jc_structured, false)
            assert_eq!(.message, "panicked at src/main.rs:41")
            assert_eq!(.jc_component, "context-gateway-gateway")
        """}]}],
    },
]


@requires_docker
def test_the_transform_parses_rfc_3339_and_the_json_fields(config, tmp_path):
    """Vector's own unit tests, run against the rendered transform. The three cases are the
    ones that decide whether a record is evidence: a good one keeps its instant and its
    fields, a bad instant is tagged rather than silently replaced, and a line that is not
    JSON at all is still shipped instead of dropped."""
    result = run_vector(tmp_path, {**config, "tests": UNIT_TESTS}, "test")
    assert result.returncode == 0, result.stdout + result.stderr


def test_only_the_three_audited_components_are_read(config):
    """A record joins the trail because of the pod it came from, so the selector is the
    whole membership rule. It is evaluated by the Kubernetes API: a pod that does not match
    is never read."""
    selector = config["sources"]["audit"]["extra_label_selector"]
    assert selector == "app.kubernetes.io/name in (context-gateway-gateway,keycloakx,gitea)"


def test_the_sink_writes_to_the_configured_bucket_and_nowhere_else(config):
    sinks = config["sinks"]
    assert list(sinks) == ["audit_store"]
    sink = sinks["audit_store"]
    assert sink["type"] == "aws_s3"
    assert sink["bucket"] == "jc-audit"
    assert sink["key_prefix"] == "audit/{{ jc_component }}/%F/"
    assert sink["encoding"]["codec"] == "json"


def test_a_sink_outage_backs_up_rather_than_dropping_records(config):
    """`when_full: drop_newest` would lose exactly the records written during the incident
    the trail exists to reconstruct."""
    buffer = config["sinks"]["audit_store"]["buffer"]
    assert buffer["type"] == "disk"
    assert buffer["when_full"] == "block"


def test_the_write_credential_is_a_reference_only(production):
    """The collector reads its credential from a Secret the operator supplies. A rendered
    key would be a credential in Git, in the manifest and in every render artifact (CC-06)."""
    daemonset = one(production, "DaemonSet", "audit-logging-collector-vector")
    container = daemonset["spec"]["template"]["spec"]["containers"][0]
    refs = [e["secretRef"]["name"] for e in container.get("envFrom", []) if "secretRef" in e]
    assert "audit-logging-s3" in refs
    rendered = yaml.safe_dump(daemonset)
    assert "secretKeyRef" not in rendered or "audit-logging-s3" in rendered
    assert "AKIA" not in rendered


def test_the_collector_runs_unprivileged_and_pinned(production):
    daemonset = one(production, "DaemonSet", "audit-logging-collector-vector")
    pod = daemonset["spec"]["template"]["spec"]
    assert pod["securityContext"]["runAsNonRoot"] is True
    container = pod["containers"][0]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["securityContext"]["allowPrivilegeEscalation"] is False
    assert "@sha256:" in container["image"]


def test_the_collector_reads_only_the_node_logs_from_the_host(production):
    """T-4775: the `kubernetes_logs` source reads /var/log/pods, where the kubelet keeps the log
    bodies on containerd (k3s, kind). The node's /var/lib holds every container's root
    filesystem and the kubelet's pod volumes, Secrets among them, so it is not mounted; the
    only other host path is the chart's own data directory, the one writable mount."""
    pod = one(production, "DaemonSet", "audit-logging-collector-vector")["spec"]["template"]["spec"]
    host = {v["name"]: v["hostPath"]["path"].rstrip("/") for v in pod["volumes"] if "hostPath" in v}
    mounts = {m["name"]: m for m in pod["containers"][0]["volumeMounts"] if m["name"] in host}
    data = [n for n, m in mounts.items() if m["mountPath"].rstrip("/") == "/vector-data-dir"]
    assert len(data) == 1, mounts
    assert host[data[0]] == "/var/lib/vector"
    assert sorted(p for n, p in host.items() if n not in data) == ["/var/log"]
    assert all(m.get("readOnly") is True for n, m in mounts.items() if n not in data), mounts


def test_the_component_is_off_until_a_bucket_is_named(local):
    """A collector shipping into a bucket that does not exist looks healthy while the trail
    goes nowhere, so an installation that has not named one runs no collector at all."""
    names = [d["metadata"]["name"] for d in local if d.get("kind") == "DaemonSet"]
    assert "audit-logging-collector-vector" not in names


@pytest.mark.parametrize(
    "overrides, message",
    [
        ({"retentionDays": 30}, "90 calendar days"),
        ({"objectLockMode": "GOVERNANCE"}, "an audit trail an administrator can shorten"),
        ({"bucket": ""}, "no bucket is named"),
    ],
    ids=["retention-below-the-requirement", "lock-mode-an-admin-can-shorten", "no-bucket"],
)
def test_a_configuration_that_would_not_meet_the_requirement_stops_the_render(own_tree, overrides, message):
    """All three are silent at apply time: the collector starts, reports healthy, and the
    trail it writes is one nobody can rely on. They are refused while they are still text."""
    if shutil.which("helmfile") is None:
        pytest.skip("helmfile not installed")
    collector = {
        "enabled": True,
        "bucket": overrides.get("bucket", "jc-audit"),
        "region": "eu-central-1",
        "endpointURL": "https://s3.example.org",
        "retentionDays": overrides.get("retentionDays", 90),
        "objectLockMode": overrides.get("objectLockMode", "COMPLIANCE"),
    }
    env_dir = own_tree / "deployment/environments/testing"
    shutil.rmtree(env_dir, ignore_errors=True)
    env_dir.mkdir(parents=True)
    (env_dir / "global.yaml.gotmpl").write_text(
        yaml.safe_dump({"audit-logging": {"collector": collector}})
    )
    try:
        result = subprocess.run(
            ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "testing", "template",
             "--skip-deps", "-q", "--selector", "component=audit-logging"],
            cwd=str(own_tree), capture_output=True, text=True,
        )
    finally:
        shutil.rmtree(env_dir, ignore_errors=True)
    assert result.returncode != 0, result.stdout
    assert message in result.stderr, result.stderr


GOLDEN = Path(__file__).resolve().parent.parent / "ci" / "golden"


def runs_the_audit_collector(environment):
    """The golden list names objects as <namespace>/<name>; each environment has its own namespace."""
    return any(
        line.startswith("apps/v1/DaemonSet ") and line.endswith("/audit-logging-collector-vector")
        for line in (GOLDEN / f"{environment}.txt").read_text().splitlines()
    )


def test_the_collector_matcher_finds_the_rendered_production_collector():
    assert runs_the_audit_collector("production")


@pytest.mark.xfail(
    strict=True,
    reason="T-5570: dev and smartcity run no audit-logging until the owner admits the "
    "S3 Object Lock bucket (OPS-42 holds only in the rendered production profile)",
)
@pytest.mark.parametrize("environment", ["dev", "smartcity"])
def test_production_profiles_run_the_append_only_sink(environment):
    """OPS-42: every environment people use ships its audit trail to the Object Lock bucket.

    The golden object list is the render the gate keeps true, so the collector missing from it
    is the collector missing from the cluster.
    """
    assert runs_the_audit_collector(environment)
