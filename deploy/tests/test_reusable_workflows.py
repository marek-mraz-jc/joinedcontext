"""The reusable workflows other repositories call (T-0010…T-0016).

Two properties are worth a test and the rest is YAML nobody can check without a runner.

The first is drift: `reusable-rust-security.yml` and `reusable-gitleaks.yml` carry a copy of
the shared configuration inside them, because a caller's GITHUB_TOKEN cannot check out this
private repository to read it. A copy that quietly stops matching its source is a gate that
enforces something nobody wrote down, so the shell that writes the copy is executed here and
compared with the file it came from.

The second is the ruleset itself. A regex that matches nothing passes every scan, which reads
exactly like a clean repository. The secret shapes are built at test time and never committed,
so this file adds nothing for the scanner to find.
"""

import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest
import yaml
from conftest import DEPLOY_TOOLS_ACTION, DEPLOY_WORKFLOWS

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = DEPLOY_WORKFLOWS
SHARED = ROOT / "ci/shared"

# The four shapes the platform's own rules exist for, none of them real. Assembled at run
# time from parts, because a literal of the right shape in this file is a finding of the very
# scan it tests (T-0542): the whole history is scanned, and a shape that once sat here stays
# in it (.gitleaksignore carries those fingerprints).
LEAKS = {
    "age": "AGE-SECRET-KEY-1" + "QWERTYUIOPASDFGHJKLZXCVBNM234567QWERTYUIOPASDFGHJKLZXCVBNM2",
    "argon2id": "$".join(["", "argon2id", "v=19", "m=65536,t=3,p=4", "c29tZXNhbHR2YWx1ZQ", "aGFzaHZhbHVlaGVyZQ"]),
    "jwt": ".".join(["eyJ" + "hbGciOiJSUzI1NiJ9", "eyJ" + "hdWQiOiJqYy1nYXRld2F5In0", "c2lnbmF0dXJlLXBsYWNlaG9sZGVy"]),
    "client_secret": 'client_secret: "' + "S3cr3tVal" + "ue0123456789abcdef" + '"',
}


@pytest.fixture(scope="module")
def gitleaks():
    binary = shutil.which("gitleaks")
    if not binary:
        pytest.skip("gitleaks is not installed")
    return binary


def scan(gitleaks: str, contents: str, path: str = "config.yaml") -> int:
    with tempfile.TemporaryDirectory() as work:
        Path(work, path).parent.mkdir(parents=True, exist_ok=True)
        Path(work, path).write_text(contents)
        return subprocess.run(
            [
                gitleaks, "detect", "--no-git", "--source", work,
                "--config", str(SHARED / "gitleaks.toml"), "--redact", "--exit-code", "1",
            ],
            capture_output=True,
        ).returncode


@pytest.mark.parametrize("name", sorted(LEAKS))
def test_every_platform_rule_catches_its_own_shape(gitleaks, name):
    assert scan(gitleaks, f"secret: {LEAKS[name]}\n") == 1


def test_a_manifest_that_carries_no_credential_passes(gitleaks):
    clean = "apiVersion: v1\nkind: Secret\nmetadata: {name: keycloak}\nspec:\n  secretRef: keycloak-admin\n"
    assert scan(gitleaks, clean) == 0


def test_every_action_is_pinned_by_commit():
    """A tag is mutable, so a gate that trusts one is not a gate (OPS-28, TS-23).

    Every workflow, not only the reusable ones a caller depends on (T-0816): `ci.yml` runs on
    every push to `main` and `image-ckan.yml` holds `packages: write` and `id-token: write`, so
    whoever can move a tag those lanes name signs an image and shapes what the cluster applies.
    """
    unpinned = []
    for workflow in WORKFLOWS:
        document = yaml.safe_load(workflow.read_text())
        for name, job in document["jobs"].items():
            # A job either calls a workflow or runs steps; a local `./…` call is this
            # repository at the commit already checked out, so there is no tag to move.
            entries = [job] if "uses" in job else job.get("steps", [])
            for entry in entries:
                uses = entry.get("uses")
                if uses and not uses.startswith("./") and len(uses.split("@")[-1]) != 40:
                    unpinned.append(f"{workflow.name} ({name}): {uses}")
    assert unpinned == []


# --- the digest gate the signing workflow is the other half of (T-0014) -------------------

DIGEST_CHECK = ROOT / "scripts/ci/check-image-digests.py"

PINNED = """
apiVersion: apps/v1
kind: Deployment
metadata: { name: gateway }
spec:
  template:
    spec:
      initContainers:
        - { name: wait, image: ghcr.io/x/wait@sha256:%s }
      containers:
        - { name: gateway, image: ghcr.io/x/gateway@sha256:%s }
""" % ("a" * 64, "b" * 64)

MUTABLE = """
apiVersion: apps/v1
kind: StatefulSet
metadata: { name: broker }
spec:
  template:
    spec:
      containers:
        - { name: broker, image: ghcr.io/x/broker:main }
"""

HOOK = """
apiVersion: batch/v1
kind: Job
metadata:
  name: ping-test
  annotations: { "helm.sh/hook": test }
spec:
  template:
    spec:
      containers:
        - { name: ping, image: alpine:3.17 }
"""


def digest_check(manifests: str) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as work:
        rendered = Path(work, "rendered.yaml")
        rendered.write_text(manifests)
        return subprocess.run(
            ["python3", str(DIGEST_CHECK), str(rendered)], capture_output=True, text=True
        )


def test_a_digest_pinned_deployment_passes():
    assert digest_check(PINNED).returncode == 0


def test_a_mutable_tag_is_named_and_refused():
    result = digest_check(MUTABLE)
    assert result.returncode == 1
    assert "ghcr.io/x/broker:main" in result.stderr


def test_a_helm_test_hook_is_not_a_deployed_workload():
    """`helmfile sync` never applies one, so its image is not what the cluster runs."""
    assert digest_check(HOOK).returncode == 0


POLICY_PATTERN = """
apiVersion: kyverno.io/v1
kind: ClusterPolicy
metadata: { name: require-image-checksum }
spec:
  rules:
    - name: require-image-checksum
      validate:
        pattern:
          spec:
            containers:
              - image: '*@*'
"""


def test_a_kyverno_pattern_is_not_a_deployed_workload():
    """T-1711: the vendored require-image-checksum policy names `image: '*@*'` as a pattern;
    a workload with a mutable tag beside it is still refused."""
    assert digest_check(POLICY_PATTERN).returncode == 0
    result = digest_check(POLICY_PATTERN + "---" + MUTABLE)
    assert result.returncode == 1 and "require-image-checksum" not in result.stderr


# --- one repository, one digest (T-2270; OPS-13, AG-52) -------------------------------------

PLATFORM = "ghcr.io/marek-mraz-jc/joinedcontext-platform"

ONE_BUILD = """
apiVersion: apps/v1
kind: Deployment
metadata: { name: context-gateway }
spec:
  template:
    spec:
      containers:
        - { name: gateway, image: "%s:main@sha256:%s" }
---
apiVersion: apps/v1
kind: Deployment
metadata: { name: agent-proxy }
spec:
  template:
    spec:
      containers:
        - { name: proxy, image: "%s:main@sha256:%s" }
""" % (PLATFORM, "c" * 64, PLATFORM, "c" * 64)

TWO_BUILDS = ONE_BUILD.replace(f"{PLATFORM}:main@sha256:{'c' * 64}", f"{PLATFORM}:main@sha256:{'d' * 64}", 1)


def test_the_components_that_share_an_image_may_pin_its_digest_each():
    """Two deployments of one build is the point, not a finding."""
    assert digest_check(ONE_BUILD).returncode == 0


def test_two_digests_behind_one_repository_are_named_and_refused():
    """The drift of 2026-09-19: the gateway eleven commits ahead of the proxy it shares a binary
    with, so a fix *in the proxy* (T-1477, AG-52) shipped to the process that does not run it."""
    result = digest_check(TWO_BUILDS)
    assert result.returncode == 1
    assert PLATFORM in result.stderr
    assert "Deployment/context-gateway/gateway" in result.stderr
    assert "Deployment/agent-proxy/proxy" in result.stderr


def test_a_mutable_tag_is_reported_before_a_drift_is_guessed_at():
    """An unpinned image has no digest to compare, so that finding comes first and alone."""
    result = digest_check(MUTABLE + "\n---" + TWO_BUILDS)
    assert result.returncode == 1
    assert "ghcr.io/x/broker:main" in result.stderr
    assert "builds at once" not in result.stderr


# --- the Rego gate the manifest-lint workflow runs (T-0015) --------------------------------

POLICIES = ROOT / "policies"
FIXTURES = ROOT / "tests/fixtures/policies"


@pytest.fixture(scope="module")
def conftest_binary():
    binary = shutil.which("conftest")
    if not binary:
        pytest.skip("conftest is not installed")
    return binary


def policy_check(conftest_binary: str, manifest: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [conftest_binary, "test", "--all-namespaces", "-p", str(POLICIES), str(manifest)],
        capture_output=True,
        text=True,
    )


def test_a_workload_that_declares_all_four_passes(conftest_binary):
    assert policy_check(conftest_binary, FIXTURES / "compliant.yaml").returncode == 0


def test_a_container_saying_root_is_refused_under_a_pod_saying_non_root(conftest_binary, tmp_path):
    """Kubernetes takes the container's runAsNonRoot over the pod's: pod true and container false runs
    as root, and the gate read the pod's value alone (F-DEP2-008)."""
    manifest = yaml.safe_load((FIXTURES / "compliant.yaml").read_text())
    pod = manifest["spec"]["template"]["spec"]
    pod.setdefault("securityContext", {})["runAsNonRoot"] = True
    pod["containers"][0]["securityContext"]["runAsNonRoot"] = False
    (tmp_path / "root.yaml").write_text(yaml.safe_dump(manifest))
    result = policy_check(conftest_binary, tmp_path / "root.yaml")
    assert result.returncode == 1 and "does not set runAsNonRoot" in result.stdout, result.stdout
    del pod["containers"][0]["securityContext"]["runAsNonRoot"]
    (tmp_path / "inherits.yaml").write_text(yaml.safe_dump(manifest))
    assert policy_check(conftest_binary, tmp_path / "inherits.yaml").returncode == 0

def test_a_workload_that_declares_none_of_them_is_refused(conftest_binary):
    """One manifest, every rule: a policy that fires on nothing reads like a clean cluster."""
    result = policy_check(conftest_binary, FIXTURES / "violating.yaml")
    assert result.returncode == 1
    for expected in (
        "does not set runAsNonRoot",
        "asks for uid 0",
        "does not drop ALL capabilities",
        "declares no memory limit",
        "default ServiceAccount",
    ):
        assert expected in result.stdout, result.stdout


def test_an_edge_whose_429_is_an_html_page_is_refused(conftest_binary):
    """T-2237: the one status the edge alone produces has to be Problem Details (API-08, GW26)."""
    result = policy_check(conftest_binary, FIXTURES / "edge-429-html.yaml")
    assert result.returncode == 1
    assert "error_page 429" in result.stdout, result.stdout
    assert "application/problem+json" in result.stdout, result.stdout


def test_an_edge_that_shapes_its_429_passes(conftest_binary):
    assert policy_check(conftest_binary, FIXTURES / "edge-429-problem-details.yaml").returncode == 0


def test_a_workload_fetching_chosen_addresses_is_refused_a_private_range(conftest_binary):
    """The runner probes URLs people type (MF-39, T-0752): a range that leaves one private
    network reachable is named, and one that excepts them all passes."""
    result = policy_check(conftest_binary, FIXTURES / "egress-open.yaml")
    assert result.returncode == 1
    assert "leaves" in result.stdout and "172.16.0.0/12" in result.stdout, result.stdout
    assert policy_check(conftest_binary, FIXTURES / "egress-closed.yaml").returncode == 0


def gate_of(conftest_binary: str, tmp_path: Path, *docs: dict) -> subprocess.CompletedProcess:
    manifest = tmp_path / "manifest.yaml"
    manifest.write_text(yaml.safe_dump_all(list(docs)))
    return policy_check(conftest_binary, manifest)


def runner_egress(*rules: dict) -> dict:
    return {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
            "metadata": {"name": "pipeline-runner"},
            "spec": {"podSelector": {"matchLabels": {"app.kubernetes.io/name": "pipeline-runner-runner"}},
                     "policyTypes": ["Egress"], "egress": list(rules)}}


def block(cidr: str, *excepted: str) -> dict:
    return {"to": [{"ipBlock": {"cidr": cidr, "except": list(excepted)}}], "ports": [{"port": 443}]}


V4_PRIVATE = ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8")


def test_an_egress_rule_without_peers_is_refused_a_chosen_address_workload(conftest_binary, tmp_path):
    """T-5702 (F-DEP2-002): a rule with no `to` reaches every address; it used to pass because
    the range check iterated over no peers."""
    result = gate_of(conftest_binary, tmp_path, runner_egress({"ports": [{"port": 443}]}))
    assert result.returncode == 1 and "without `to`" in result.stdout, result.stdout


def test_carrier_grade_nat_and_ipv6_internal_ranges_count_as_private(conftest_binary, tmp_path):
    """T-5702 (F-DEP2-010): 100.64.0.0/10 must be excepted from an IPv4 block, and an IPv6 block
    must except loopback, unique-local and link-local, which ADR-N-040 treats as private."""
    result = gate_of(conftest_binary, tmp_path, runner_egress(block("0.0.0.0/0", *V4_PRIVATE)))
    assert result.returncode == 1 and "100.64.0.0/10" in result.stdout, result.stdout
    result = gate_of(conftest_binary, tmp_path, runner_egress(block("::/0", "::1/128", "fe80::/10")))
    assert result.returncode == 1 and "fc00::/7" in result.stdout, result.stdout
    assert "10.0.0.0/8" not in result.stdout, "an IPv6 block is held to IPv6 ranges"
    closed = runner_egress(block("0.0.0.0/0", *V4_PRIVATE, "100.64.0.0/10"),
                           block("::/0", "::1/128", "fc00::/7", "fe80::/10"))
    assert gate_of(conftest_binary, tmp_path, closed).returncode == 0


def broker_callers(namespace: str, *refs: tuple) -> dict:
    return {"apiVersion": "policy.linkerd.io/v1alpha1", "kind": "MeshTLSAuthentication",
            "metadata": {"name": "context-broker-api-clients", "namespace": namespace},
            "spec": {"identityRefs": [{"kind": "ServiceAccount", "name": name, "namespace": ns}
                                      for name, ns in refs]}}


def test_a_broker_caller_counts_only_in_its_components_namespace(conftest_binary, tmp_path):
    """T-5702 (F-DEP2-007): a ServiceAccount named portal in an App namespace is not the Portal,
    and the broker believes the subject any admitted caller sends."""
    split = broker_callers("dev-context-broker", ("context-broker", "dev-context-broker"),
                           ("context-gateway", "dev-context-gateway"), ("portal", "dev-portal"))
    single = broker_callers("jc", ("context-broker", "jc"), ("context-gateway", "jc"), ("portal", "jc"))
    assert gate_of(conftest_binary, tmp_path, split, single).returncode == 0
    for ns in ("dev-x-apps", "x-apps", "dev-context-gateway"):
        result = gate_of(conftest_binary, tmp_path, broker_callers("dev-context-broker", ("portal", ns)))
        assert result.returncode == 1 and f"not {ns}" in result.stdout, (ns, result.stdout)


PINS = "scripts/open-data/sdm/pins.json"


def test_an_sdm_commit_pin_is_no_leak_and_nothing_else_shaped_like_it_is_let_through(gitleaks):
    """T-4341: generic-api-key took the pin of dataModel.KeyPerformanceIndicator, a public git
    commit, for a key. Only that line's shape, a dataModel name pinned to 40 hex, is let through."""
    pin = "0123456789abcdef" * 2 + "01234567"
    key = "Zx9" + "Qw7Er5Ty3Ui1Op8As6Df4Gh2Jk0Lm"
    assert scan(gitleaks, '{\n  "dataModel.KeyPerformanceIndicator": "%s",\n}\n' % pin, PINS) == 0
    assert scan(gitleaks, '{\n  "dataModel.KeyPerformanceIndicator": "%s",\n}\n' % key, PINS) == 1
    assert scan(gitleaks, '{"apiKey": "%s"}\n' % pin, PINS) == 1, "a commit-shaped value under another key"
    assert scan(gitleaks, '{"apiKey": "%s"}\n' % key, PINS) == 1
    assert scan(gitleaks, f"secret: {LEAKS['jwt']}\n", PINS) == 1


def test_no_workflow_or_recipe_runs_an_install_script_or_an_unpinned_package():
    """A script piped from a branch or an unpinned pip install runs whatever its host serves today,
    in jobs that hold contents: write (F-DEP1-005, F-DEP1-006, F-DEP2-012): every tool comes as a
    release asset checked against its sha256, every package by hash."""
    texts = {p: p.read_text() for p in [*DEPLOY_WORKFLOWS, DEPLOY_TOOLS_ACTION, ROOT / "justfile"]}
    for path, text in texts.items():
        # one line: a YAML block scalar (`run: |`) followed by a `sh` command is no pipe
        assert not re.search(r"\|[ \t]*(ba)?sh\b", text), path
        for line in re.findall(r"pip install[^\n]*", text):
            # The one exception installs the Model Tools checkout of ci/model-tools.lock (a commit),
            # whose own dependencies are not hash-locked yet (chyby.md, T-6218).
            local = path.name == "ci.yml" and "platform/tools/model-tools" in line
            assert "--require-hashes" in line or local, (path, line)
