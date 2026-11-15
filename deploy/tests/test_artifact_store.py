"""T-0035: the artifact store, an S3 API for everything the platform renders (PF-29…PF-33).

Two properties carry the requirements and neither is visible in a running cluster until it is
too late: object lock is settable only when the bucket is created (PF-33), and the store must
be reachable by exactly three pods and by nothing outside the cluster (PF-32, Architecture/17
§3).
"""

import json
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

CHART = Path(__file__).resolve().parent.parent / "components/artifact-store/charts/rustfs"
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")


def render(tmp_path: Path, **values) -> dict:
    payload = {
        "image": {"repository": "docker.io/rustfs/rustfs", "tag": "1.0.0-rc.5", "digest": "sha256:abc"},
        "bootstrapImage": {"repository": "docker.io/amazon/aws-cli", "tag": "2.36.40", "digest": "sha256:def"},
        "nameOverride": "artifact-store",
        **values,
    }
    tmp_path.mkdir(parents=True, exist_ok=True)
    values_file = tmp_path / "values.yaml"
    values_file.write_text(yaml.safe_dump(payload))
    result = subprocess.run(
        ["helm", "template", "artifact-store-store", str(CHART), "-f", str(values_file)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    return {d["kind"]: d for d in yaml.safe_load_all(result.stdout) if d}


def script_of(docs: dict) -> str:
    return docs["Job"]["spec"]["template"]["spec"]["containers"][0]["command"][2]


@requires_helm
def test_the_bucket_is_created_with_object_lock(tmp_path):
    """Object lock is a creation-time property: a bucket made without it can never be locked,
    and every promise about published artifacts would be a convention instead."""
    docs = render(tmp_path)
    assert "--object-lock-enabled-for-bucket" in script_of(docs)


@requires_helm
def test_a_default_retention_is_compliance_and_off_unless_asked_for(tmp_path):
    """A bucket-wide default would also lock the gateway's file cache, which has to expire."""
    without = script_of(render(tmp_path / "off"))
    assert "put-object-lock-configuration" not in without

    with_default = script_of(render(tmp_path / "on", objectLock={"enabled": True, "defaultRetentionDays": 3650}))
    assert '"Mode":"COMPLIANCE"' in with_default
    assert '"Days":3650' in with_default


@requires_helm
def test_the_file_cache_expires(tmp_path):
    docs = render(tmp_path, fileCache={"prefix": "filecache/", "expiryDays": 1})
    script = script_of(docs)
    assert "put-bucket-lifecycle-configuration" in script
    assert '"Prefix":"filecache/"' in script and '"Days":1' in script
    # Versioned by the object lock: an expiry leaves the old version and its marker behind unless
    # the rule also expires those (F-DEP1-014).
    rules = json.loads(re.search(r"--lifecycle-configuration '([^']+)'", script).group(1))["Rules"]
    assert rules[0]["NoncurrentVersionExpiration"] == {"NoncurrentDays": 1}
    assert {"ExpiredObjectDeleteMarker": True} in [r["Expiration"] for r in rules]


@requires_helm
def test_the_store_keeps_its_data_on_a_claim(tmp_path):
    docs = render(tmp_path, storage={"size": "25Gi", "storageClass": "fast"})
    claims = docs["StatefulSet"]["spec"]["volumeClaimTemplates"]
    assert len(claims) == 1
    claim = claims[0]["spec"]
    assert claim["accessModes"] == ["ReadWriteOnce"]
    assert claim["resources"]["requests"]["storage"] == "25Gi"
    assert claim["storageClassName"] == "fast"


@requires_helm
def test_the_service_never_points_at_the_bootstrap_pod(tmp_path):
    """Both pods belong to the same release and carry the same name and instance labels. A
    Service selecting only those would list the Job's pod as an endpoint and answer an S3 call
    with a container that is not the store."""
    docs = render(tmp_path)
    selector = docs["Service"]["spec"]["selector"]
    assert selector["app.kubernetes.io/component"] == "store"
    job_labels = docs["Job"]["spec"]["template"]["metadata"]["labels"]
    assert job_labels["app.kubernetes.io/component"] == "bucket"


@requires_helm
def test_no_credential_is_ever_templated_into_a_manifest(tmp_path):
    """The keys live in a generated Secret; the store and the bootstrap read them by
    reference, and neither values nor manifests may carry the value itself."""
    docs = render(tmp_path)
    store = docs["StatefulSet"]["spec"]["template"]["spec"]["containers"][0]
    job = docs["Job"]["spec"]["template"]["spec"]["containers"][0]
    for container, names in ((store, ("RUSTFS_ACCESS_KEY", "RUSTFS_SECRET_KEY")),
                             (job, ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"))):
        env = {e["name"]: e for e in container["env"]}
        for name in names:
            assert "value" not in env[name], name
            assert env[name]["valueFrom"]["secretKeyRef"]["name"] == "artifact-store-root"


@requires_helm
def test_the_store_runs_locked_down_and_asks_for_nothing_from_the_api(tmp_path):
    docs = render(
        tmp_path,
        podSecurityContext={"runAsNonRoot": True, "runAsUser": 1000, "fsGroup": 1000},
        securityContext={"readOnlyRootFilesystem": True, "allowPrivilegeEscalation": False},
    )
    pod = docs["StatefulSet"]["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False
    assert docs["ServiceAccount"]["automountServiceAccountToken"] is False
    assert pod["securityContext"]["fsGroup"] == 1000
    container = pod["containers"][0]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    # A read-only root filesystem needs the two paths the process writes to.
    assert {m["mountPath"] for m in container["volumeMounts"]} == {"/data", "/logs", "/tmp"}
    env = {e["name"]: e.get("value") for e in container["env"]}
    # Without this an identity that may write an object may encrypt it under any KMS key.
    assert env["RUSTFS_KMS_ENFORCE_SSE_KEY_POLICY"] == "true"
    assert env["RUSTFS_CONSOLE_ENABLE"] == "false"


@requires_helm
def test_the_meshed_bootstrap_can_finish(tmp_path):
    docs = render(tmp_path, serviceMesh={"enabled": True})
    annotations = docs["Job"]["spec"]["template"]["metadata"]["annotations"]
    assert annotations["config.alpha.linkerd.io/proxy-enable-native-sidecar"] == "true"


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


def test_only_the_gateway_the_portal_and_the_bootstrap_reach_the_store(local):
    """PF-32: no pre-signed URLs and no ingress route, so the network is the access model
    until per-organization credentials are issued from OpenBao."""
    policies = {d["metadata"]["name"]: d for d in local
                if d.get("kind") == "NetworkPolicy" and "artifact-store" in d["metadata"]["name"]}
    store = policies["artifact-store"]
    assert store["spec"]["podSelector"]["matchLabels"] == {"app.kubernetes.io/component": "store"}
    allowed = store["spec"]["ingress"][0]["from"]
    names = set()
    for peer in allowed:
        labels = peer["podSelector"]["matchLabels"]
        names.add(labels.get("app.kubernetes.io/name") or labels.get("app.kubernetes.io/component") or labels["cnpg.io/cluster"])
    # T-5289: the Postgres instances and the restore drill's copy, for the backup bucket alone.
    assert names == {"context-gateway-gateway", "portal-portal", "bucket", "postgres-cluster", "postgres-cluster-drill"}
    assert [p["port"] for p in store["spec"]["ingress"][0]["ports"]] == [9000]
    # The store calls nothing but DNS.
    assert all("kube-dns" in str(rule) for rule in store["spec"]["egress"])


def test_the_store_is_not_published(local):
    """An Ingress or an APISIX route here would put bytes in front of a browser without a
    policy decision in between."""
    for doc in local:
        name = doc.get("metadata", {}).get("name", "")
        if doc.get("kind") == "Ingress":
            assert "artifact" not in name and "rustfs" not in name
    routes = [d for d in local if d.get("kind") == "ConfigMap" and "apisix" in d["metadata"]["name"]]
    assert not [r for r in routes if "artifact-store" in yaml.safe_dump(r.get("data", {}))]


def test_the_image_is_pinned_by_digest(local):
    store = [d for d in local if d.get("kind") == "StatefulSet" and d["metadata"]["name"] == "artifact-store"]
    assert len(store) == 1
    assert "@sha256:" in store[0]["spec"]["template"]["spec"]["containers"][0]["image"]
    job = [d for d in local if d.get("kind") == "Job" and d["metadata"]["name"] == "artifact-store-bucket"]
    assert "@sha256:" in job[0]["spec"]["template"]["spec"]["containers"][0]["image"]
