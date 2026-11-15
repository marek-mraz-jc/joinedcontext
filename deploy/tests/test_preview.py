"""Code-branch previews on dev (T-5548, ADR-N-062, docs Deployment/17).

The chart components/preview/charts/preview renders one preview namespace: these tests hold its
render to kubeconform, the admission policies and the golden object list, and its NetworkPolicy,
quota and pods to the isolation design. The dev render must carry the other halves (pg_hba, the
callee NetworkPolicies, the reaper) and no production render may. `scripts/preview.sh check` and
`just dev-preview` are run against a stub kubectl and must refuse a node above the memory line.
"""

import json
import os
import shutil
import subprocess
import textwrap

import pytest
from conftest import PROJECT_ROOT, load_yaml_all, set_global
from test_dev_recipes import write_nodes

CHART = PROJECT_ROOT / "components/preview/charts/preview"
GOLDEN = PROJECT_ROOT / "ci/golden/preview.txt"
PORTAL = "ghcr.io/marek-mraz-jc/joinedcontext-portal@sha256:" + "a" * 64
UI = "ghcr.io/marek-mraz-jc/joinedcontext-portal-ui@sha256:" + "b" * 64
LABEL = "sandbox.joinedcontext.com/lifecycle"

needs_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")


def helm_template(*sets: str) -> subprocess.CompletedProcess:
    args = ["helm", "template", "pv-fix-login", str(CHART)]
    for value in ("slug=fix-login", f"images.portal={PORTAL}", f"images.portalUi={UI}", *sets):
        args += ["--set-string", value]
    return subprocess.run(args, capture_output=True, text=True, check=False)


@pytest.fixture(scope="module")
def render(tmp_path_factory):
    if shutil.which("helm") is None:
        pytest.skip("helm not installed")
    result = helm_template()
    assert result.returncode == 0, result.stderr
    out = tmp_path_factory.mktemp("preview") / "preview.yaml"
    out.write_text(result.stdout)
    return out


@pytest.fixture(scope="module")
def docs(render):
    return [d for d in load_yaml_all(render.read_text()) if isinstance(d, dict)]


def one(docs, kind, name):
    found = [d for d in docs if d["kind"] == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind}/{name}: {len(found)}"
    return found[0]


def test_the_render_passes_kubeconform_strict(render):
    if shutil.which("kubeconform") is None:
        pytest.skip("kubeconform not installed")
    result = subprocess.run(["kubeconform", "-strict", "-summary", str(render)], capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stdout + result.stderr


def test_the_render_passes_the_policy_gate_and_pins_every_image(render):
    if shutil.which("conftest") is None:
        pytest.skip("conftest not installed")
    gate = subprocess.run(["conftest", "test", "--all-namespaces", "-p", str(PROJECT_ROOT / "policies"), str(render)],
                          capture_output=True, text=True, check=False)
    assert gate.returncode == 0, gate.stdout[-3000:]
    pins = subprocess.run(["python3", str(PROJECT_ROOT / "scripts/ci/check-image-digests.py"), str(render)],
                          capture_output=True, text=True, check=False)
    assert pins.returncode == 0, pins.stdout + pins.stderr


def test_the_render_matches_its_golden_object_list(render):
    listed = subprocess.run(["python3", str(PROJECT_ROOT / "ci/golden/list-objects.py"), str(render)],
                            capture_output=True, text=True, check=True).stdout
    assert listed == GOLDEN.read_text(), (
        "the preview chart's objects changed: review the diff and write ci/golden/preview.txt with "
        "python3 ci/golden/list-objects.py over this test's render (helm_template())")


def test_the_namespace_is_a_reaped_restricted_sandbox(docs):
    ns = one(docs, "Namespace", "pv-fix-login")
    assert ns["metadata"]["labels"][LABEL] == "preview"
    assert ns["metadata"]["labels"]["pod-security.kubernetes.io/enforce"] == "restricted"
    assert ns["metadata"]["annotations"]["sandbox.joinedcontext.com/ttl"] == "48h"
    assert ns["metadata"]["annotations"]["config.linkerd.io/default-inbound-policy"] == "cluster-authenticated"
    assert {d["metadata"].get("namespace") for d in docs if d["kind"] != "Namespace"} == {"pv-fix-login"}


def test_the_quota_and_limits_keep_a_preview_small(docs):
    hard = one(docs, "ResourceQuota", "preview")["spec"]["hard"]
    assert hard["requests.memory"] == "1Gi" and hard["limits.memory"] == "2Gi" and hard["pods"] == "4"
    assert hard["services.loadbalancers"] == "0" and hard["services.nodeports"] == "0"
    limit = one(docs, "LimitRange", "preview")["spec"]["limits"][0]
    assert limit["max"]["memory"] == "1Gi", "a 2Gi container is refused at admission"
    assert limit["default"]["memory"] == "1Gi"


def peers(rules, key):
    return [json.dumps(peer, sort_keys=True) for rule in rules for peer in rule[key]]


def test_the_network_policy_opens_only_the_designed_paths(docs):
    assert one(docs, "NetworkPolicy", "default-deny")["spec"] == {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]}
    spec = one(docs, "NetworkPolicy", "preview")["spec"]
    egress = spec["egress"]
    rendered = json.dumps(spec)
    assert "ipBlock" not in rendered, "no address outside the cluster"
    for dev_service in ("context-gateway", "context-broker", "portal-portal", "gitea", "apisix"):
        assert dev_service not in rendered, f"the preview reaches dev's {dev_service}"
    dev = {"kubernetes.io/metadata.name": "dev"}
    reached = {(json.dumps(p.get("podSelector", {}).get("matchLabels"), sort_keys=True), p.get("namespaceSelector", {}).get("matchLabels") == dev)
               for rule in egress for p in rule["to"]}
    assert (json.dumps({"app.kubernetes.io/instance": "keycloak-app"}), True) in reached
    assert (json.dumps({"cnpg.io/cluster": "postgres-cluster"}), True) in reached
    assert sum(1 for _, in_dev in reached if in_dev) == 2, "dev's Keycloak and Postgres and nothing else of dev"
    ports = {(json.dumps(r["to"], sort_keys=True), p["port"]) for r in egress for p in r["ports"]}
    assert all(port in (53, 4143, 5432, 8080, 8086, 8090, 8443) for _, port in ports)


def test_the_pods_hold_no_api_token_and_no_secret_but_their_own(docs):
    for name in ("portal", "portal-ui"):
        pod = one(docs, "Deployment", name)["spec"]["template"]["spec"]
        assert pod["automountServiceAccountToken"] is False
        assert all("secret" not in v and "projected" not in v for v in pod["volumes"])
        for container in pod["containers"]:
            refs = {e["valueFrom"]["secretKeyRef"]["name"] for e in container.get("env", []) if "valueFrom" in e}
            assert refs <= {"preview-credentials"}
            assert "@sha256:" in container["image"]
    env = {e["name"]: e.get("value") for e in one(docs, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_OIDC_ISSUER"] == "https://idm.dev.joinedcontext.com/realms/preview"
    assert env["JC_OIDC_CLIENT_ID"] == "pv-fix-login-portal"
    assert env["JC_PORTAL_DATABASE_URL"].endswith("/pv_fix_login") and "//pv_fix_login:" in env["JC_PORTAL_DATABASE_URL"]


def test_one_host_one_http01_certificate(docs):
    ingress = one(docs, "Ingress", "preview")
    assert ingress["metadata"]["annotations"]["cert-manager.io/cluster-issuer"] == "letsencrypt-prod"
    assert ingress["spec"]["tls"] == [{"hosts": ["preview-fix-login.dev.joinedcontext.com"], "secretName": "preview-tls"}]
    assert "*" not in json.dumps(ingress)


@needs_helm
@pytest.mark.parametrize("value, says", [
    ("slug=Fix", "slug"), ("slug=a_b", "slug"), ("slug=" + "a" * 41, "slug"), ("slug=-a", "slug"),
    ("images.portal=ghcr.io/marek-mraz-jc/joinedcontext-portal:main", "/images/portal"),
    ("images.portal=docker.io/library/nginx@sha256:" + "c" * 64, "/images/portal"),
    ("ttl=forever", "ttl"),
], ids=["uppercase", "underscore", "41-chars", "leading-dash", "tag-not-digest", "foreign-registry", "ttl"])
def test_the_chart_refuses_a_bad_value(value, says):
    result = helm_template(value)
    assert result.returncode != 0 and says in result.stderr, result.stderr


# ---- the dev render's halves ------------------------------------------------------------------

def test_dev_with_branch_previews_on_carries_the_other_halves(rendered_variant):
    """The dev side of a preview, rendered as dev would be once global.branchPreviews is on: pg_hba
    holds pv_* roles to their own database before the catch-all, and only Keycloak and Postgres
    admit the preview namespaces."""
    docs = rendered_variant("dev", lambda tree: set_global(tree, "branchPreviews.enabled", True))
    cluster = next(d for d in docs if d["kind"] == "Cluster" and d["metadata"]["name"] == "postgres-cluster")
    hba = cluster["spec"]["postgresql"]["pg_hba"]
    own, rest, catch_all = (hba.index("hostssl sameuser +pv_previews all scram-sha-256"),
                            hba.index("host all +pv_previews all reject"), hba.index("hostssl all all all scram-sha-256"))
    assert hba.index("hostnossl all all all reject") < own < rest < catch_all
    group = next(r for r in cluster["spec"]["managed"]["roles"] if r["name"] == "pv_previews")
    assert group["login"] is False and group["superuser"] is False
    policies = {d["metadata"]["name"]: d for d in docs if d["kind"] == "NetworkPolicy"}
    opened = sorted(n for n, d in policies.items() if LABEL in json.dumps(d["spec"].get("ingress", [])))
    assert opened == ["keycloak-from-branch-previews", "postgres-from-branch-previews"]
    assert [p["port"] for p in policies["postgres-from-branch-previews"]["spec"]["ingress"][0]["ports"]] == [5432, 4143]


def test_dev_keeps_branch_previews_off_while_postgres_has_no_room(rendered):
    """T-5548: two previews need max_connections 67, over the 2Gi pod's memory budget."""
    docs = rendered("dev")
    assert "pv_previews" not in json.dumps([d for d in docs if d["kind"] == "Cluster"])
    assert not [d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-from-branch-previews")]


def test_dev_runs_the_reaper_and_its_preview_name_rule(rendered):
    kinds = {(d["kind"], d["metadata"]["name"]) for d in rendered("dev")}
    assert ("CronJob", "sandbox-reaper-reaper") in kinds
    assert ("ValidatingAdmissionPolicy", "sandbox-reaper-reaper-preview-names") in kinds


@pytest.mark.parametrize("env", ["production", "smartcity"])
def test_no_production_render_opens_a_path_to_previews(rendered, env):
    docs = rendered(env)
    assert "pv_previews" not in json.dumps([d for d in docs if d["kind"] == "Cluster"])
    assert not [d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-from-branch-previews")]


# ---- the recipe's guards ----------------------------------------------------------------------

STUB = textwrap.dedent("""\
    #!/usr/bin/env bash
    # kubectl as the recipe's guards ask it: answers from STUB_* variables, records nothing else.
    case "$*" in
      "config view --minify -o jsonpath={.clusters[0].cluster.server}") echo "https://203.0.113.10:6443" ;;
      *"get clusters.postgresql.cnpg.io postgres-cluster"*) echo "${STUB_ROLES:-portal pv_previews}" ;;
      "get cronjobs -A -o name") echo "${STUB_CRONJOBS:-cronjob.batch/sandbox-reaper-reaper}" ;;
      "top nodes --no-headers") echo "node-1 900m 11% ${STUB_MEMORY_BYTES:-12000Mi} ${STUB_MEMORY:-50%}" ;;
      *"get namespaces -l sandbox.joinedcontext.com/lifecycle=preview -o name"*) printf '%s' "${STUB_PREVIEWS:-}" ;;
      *"get certificates.cert-manager.io -A"*) printf '%s' "${STUB_CERTIFICATES:-}" ;;
      *) echo "stub kubectl: unexpected call: $*" >&2; exit 3 ;;
    esac
    """)


@pytest.fixture
def stub_env(tmp_path):
    (tmp_path / "kubectl").write_text(STUB)
    (tmp_path / "kubectl").chmod(0o755)
    return {**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}", "JUST_NO_DOTENV": "true",
            "JC_ENVIRONMENTS": str(write_nodes(tmp_path / "environments"))}


def check(stub_env, slug="fix-login", **stub):
    env = {**stub_env, **{f"STUB_{k.upper()}": v for k, v in stub.items()}}
    return subprocess.run([str(PROJECT_ROOT / "scripts/preview.sh"), "check", slug], env=env,
                          capture_output=True, text=True, check=False, cwd=str(PROJECT_ROOT))


def test_the_guards_pass_a_quiet_node(stub_env):
    result = check(stub_env, memory="89%", previews="namespace/pv-other\n")
    assert result.returncode == 0, result.stderr
    assert "1 of 2 previews" in result.stdout


@pytest.mark.parametrize("memory", ["90%", "94%", "<unknown>"])
def test_the_guards_refuse_a_node_at_or_above_90_percent(stub_env, memory):
    result = check(stub_env, memory=memory)
    assert result.returncode == 1 and "a preview starts only below 90%" in result.stderr, result.stderr


@pytest.mark.parametrize("stub, says", [
    ({"roles": "portal keycloak"}, "branch previews are off on this cluster"),
    ({"cronjobs": "cronjob.batch/other"}, "the sandbox reaper is not deployed"),
    ({"previews": "namespace/pv-a\nnamespace/pv-b\n"}, "2 branch previews run already"),
    ({"certificates": "".join(f"pv-x{i} 2999-01-01T00:00:00Z\n" for i in range(10))}, "the budget is 10"),
], ids=["previews-off", "no-reaper", "cap", "certificates"])
def test_the_guards_refuse(stub_env, stub, says):
    result = check(stub_env, **stub)
    assert result.returncode == 1 and says in result.stderr, result.stderr


def test_a_rerun_of_a_running_preview_does_not_count_itself(stub_env):
    result = check(stub_env, previews="namespace/pv-fix-login\nnamespace/pv-b\n",
                   certificates="pv-fix-login 2999-01-01T00:00:00Z\n")
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("slug", ["Fix", "a_b", "x" * 41, "", "a;rm"])
def test_the_recipe_refuses_a_bad_slug_before_any_call(stub_env, slug):
    result = check(stub_env, slug=slug)
    assert result.returncode == 1 and "is not a preview slug" in result.stderr, result.stderr


def test_just_dev_preview_refuses_above_the_memory_line(stub_env):
    if shutil.which("just") is None:
        pytest.skip("just not installed")
    result = subprocess.run(["just", "dev-preview", "x"], env={**stub_env, "STUB_MEMORY": "91%"},
                            capture_output=True, text=True, check=False, cwd=str(PROJECT_ROOT))
    assert result.returncode != 0 and "node node-1 uses 91% of its memory" in result.stderr, result.stderr


def test_just_dev_preview_refuses_any_context_but_dev(stub_env, tmp_path):
    if shutil.which("just") is None:
        pytest.skip("just not installed")
    stub = tmp_path / "kubectl"
    stub.write_text(STUB.replace("https://203.0.113.10:6443", "https://10.0.0.1:6443"))
    result = subprocess.run(["just", "dev-preview", "x"], env=stub_env, capture_output=True, text=True,
                            check=False, cwd=str(PROJECT_ROOT))
    assert result.returncode != 0 and "not the dev cluster" in result.stderr, result.stderr
