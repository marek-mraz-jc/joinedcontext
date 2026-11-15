"""The policies this chart deploys, judged as the chart renders them.

Two groups. The CNPG label-protection policies must let the operator's own initdb Job
through and still block anyone else claiming those labels (T-0232 / T-0234). The Pod
Security Standards policies must hold for the workloads this deployment owns and stop at
its namespaces (T-0008)."""

import os
import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHART = PROJECT_ROOT / "components/runtime-policies/charts/runtime-policies"
FIXTURES = Path(__file__).resolve().parent / "fixtures"
CNPG_SA = "system:serviceaccount:dev:postgres-operator-cloudnative-pg"
JOB_CONTROLLER_SA = "system:serviceaccount:kube-system:job-controller"

requires_kyverno = pytest.mark.skipif(shutil.which("kyverno") is None, reason="kyverno CLI not installed")
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")


@pytest.fixture(scope="module")
def policies(tmp_path_factory):
    """The two operator policies as the chart renders them for an enforcing environment."""
    out = tmp_path_factory.mktemp("policies") / "policies.yaml"
    rendered = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "failureAction=Enforce", "--set", "linkerd.enabled=false",
         "--set", "cnpgOperatorNamespace=dev", "--namespace", "dev"],
        capture_output=True, text=True, check=True,
    ).stdout
    out.write_text(rendered)
    return out


def userinfo(tmp_path: Path, username: str) -> Path:
    path = tmp_path / "userinfo.yaml"
    path.write_text(
        textwrap.dedent(
            f"""\
            apiVersion: cli.kyverno.io/v1alpha1
            kind: UserInfo
            metadata:
              name: user
            userInfo:
              username: {username}
              groups: [system:serviceaccounts, system:authenticated]
            """
        )
    )
    return path


def failed_rules(policies: Path, resource: Path, username: str, tmp_path: Path) -> set:
    """Names of the rules `kyverno apply` reports as failed for this creator."""
    result = subprocess.run(
        ["kyverno", "apply", str(policies), "--resource", str(resource),
         "--userinfo", str(userinfo(tmp_path, username))],
        capture_output=True, text=True, check=False,
    )
    assert result.returncode in (0, 1), result.stderr or result.stdout
    return set(re.findall(r"^\d+ - (\S+) ", result.stdout, re.M))


@requires_kyverno
@requires_helm
def test_cnpg_initdb_job_and_its_pod_are_allowed(policies, tmp_path):
    """The operator creates the Job; the Job controller creates its Pod. Both pass."""
    assert failed_rules(policies, FIXTURES / "cnpg-initdb-job.yaml", CNPG_SA, tmp_path) == set()
    assert failed_rules(policies, FIXTURES / "cnpg-initdb-job.yaml", JOB_CONTROLLER_SA, tmp_path) == set()


@requires_kyverno
@requires_helm
def test_a_foreign_identity_may_not_claim_the_cnpg_labels(policies, tmp_path):
    # Kyverno autogen mirrors the Pod rules onto the Job, which is what keeps the
    # job-controller exemption honest.
    failed = failed_rules(policies, FIXTURES / "spoofed-cnpg-pod.yaml",
                          "system:serviceaccount:dev:attacker", tmp_path)
    assert {"cnpg-labels-only-from-cnpg", "cnpg-jobrole-only-from-cnpg",
            "autogen-cnpg-labels-only-from-cnpg", "autogen-cnpg-jobrole-only-from-cnpg"} <= failed


POD_SECURITY_RULES = {"require-drop-all", "validate-readOnlyRootFilesystem",
                      "run-as-non-root-user", "run-as-non-root"}


@pytest.fixture(scope="module")
def pod_security(tmp_path_factory):
    """The Pod Security policies as the chart renders them for the `dev` deployment.

    Rendered rather than read from files/upstream, because two of the things worth testing
    — that the policies stop at this deployment's namespaces, and that the action is the
    environment's — exist only after the template has run."""
    out = tmp_path_factory.mktemp("pod-security") / "policies.yaml"
    rendered = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "failureAction=Enforce", "--set", "linkerd.enabled=false",
         "--set", "operators.enabled=false", "--set", "podSecurity.namespaces={dev}",
         "--namespace", "dev"],
        capture_output=True, text=True, check=True,
    ).stdout
    out.write_text(rendered)
    return out


@requires_helm
def test_the_rendered_policies_carry_the_environment_action_and_stop_at_its_namespaces(pod_security):
    """The vendored copies match every namespace and carry the deprecated
    spec.validationFailureAction; neither may survive into what gets applied."""
    docs = [d for d in yaml.safe_load_all(pod_security.read_text()) if d]
    by_name = {d["metadata"]["name"]: d for d in docs}
    assert {"drop-all-capabilities", "require-ro-rootfs",
            "require-run-as-non-root-user", "require-run-as-nonroot"} <= set(by_name)
    for name in ("drop-all-capabilities", "require-ro-rootfs",
                 "require-run-as-non-root-user", "require-run-as-nonroot"):
        policy = by_name[name]
        assert "validationFailureAction" not in policy["spec"], f"{name} kept the deprecated field"
        for rule in policy["spec"]["rules"]:
            assert rule["validate"]["failureAction"] == "Enforce", f"{name}/{rule['name']}"
            for matcher in rule["match"]["any"]:
                assert matcher["resources"]["namespaces"] == ["dev"], f"{name}/{rule['name']}"


@requires_helm
def test_no_pod_security_policy_is_rendered_without_a_namespace_to_apply_it_to(tmp_path):
    """A ClusterPolicy with no namespace scope matches every namespace, including the ones
    the cluster distribution owns. Rendering nothing is the safe answer."""
    rendered = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "failureAction=Enforce", "--set", "linkerd.enabled=false",
         "--set", "operators.enabled=false", "--namespace", "dev"],
        capture_output=True, text=True, check=True,
    ).stdout
    names = {d["metadata"]["name"] for d in yaml.safe_load_all(rendered) if d}
    assert "drop-all-capabilities" not in names, "scopeless Pod Security policy rendered"


@requires_kyverno
@requires_helm
def test_a_hardened_pod_passes_every_pod_security_policy(pod_security, tmp_path):
    assert failed_rules(pod_security, FIXTURES / "hardened-pod.yaml", CNPG_SA, tmp_path) == set()


@requires_kyverno
@requires_helm
def test_an_unhardened_pod_fails_every_pod_security_policy(pod_security, tmp_path):
    failed = failed_rules(pod_security, FIXTURES / "unhardened-pod.yaml", CNPG_SA, tmp_path)
    assert POD_SECURITY_RULES <= failed, f"missed: {sorted(POD_SECURITY_RULES - failed)}"


@requires_kyverno
@requires_helm
def test_the_same_pod_outside_this_deployment_is_not_judged(pod_security, tmp_path):
    """`traefik` belongs to the cluster distribution. Its pods are none of our business, and
    a policy that blocked its upgrades would be worse than the gap it closes."""
    failed = failed_rules(pod_security, FIXTURES / "unhardened-pod-other-namespace.yaml",
                          CNPG_SA, tmp_path)
    assert not (POD_SECURITY_RULES & failed), f"judged outside its scope: {sorted(failed)}"


@requires_kyverno
@requires_helm
def test_the_linkerd_init_container_may_still_add_the_capabilities_it_needs(pod_security, tmp_path):
    """It drops ALL and adds back the two it needs for iptables. disallow-capabilities-strict
    would reject it, and with it every meshed pod on the cluster."""
    assert failed_rules(pod_security, FIXTURES / "linkerd-meshed-pod.yaml", CNPG_SA, tmp_path) == set()


def mutated(policies: Path, resource: Path, tmp_path: Path) -> dict:
    """The resource as admission would let it through, after the chart's mutations."""
    out = tmp_path / "mutated"
    subprocess.run(
        ["kyverno", "apply", str(policies), "--resource", str(resource), "-o", str(out)],
        capture_output=True, text=True, check=False,
    )
    written = list(out.rglob("*.yaml"))
    assert len(written) == 1, f"expected one mutated resource, got {written}"
    # kyverno writes the patched resource followed by a document separator
    return next(d for d in yaml.safe_load_all(written[0].read_text()) if d)


@requires_kyverno
@requires_helm
def test_a_pod_that_never_asked_for_a_token_does_not_get_one(pod_security, tmp_path):
    """The upstream charts of apisix, Keycloak and keycloak-config-cli expose no
    automountServiceAccountToken field at all, so the pod is where it has to be set."""
    pod = mutated(pod_security, FIXTURES / "hardened-pod.yaml", tmp_path)
    assert pod["spec"]["automountServiceAccountToken"] is False


@requires_kyverno
@requires_helm
def test_a_token_the_api_server_injected_is_taken_away(pod_security, tmp_path):
    """The ServiceAccount admission plugin mounts kube-api-access-* before Kyverno sees the Pod,
    so setting the field alone left the token mounted and the projected-token check refused
    every such Pod: no apisix pod in any k3d job of ci-full 41294209744."""
    fixture = FIXTURES / "api-token-injected-pod.yaml"
    pod = mutated(pod_security, fixture, tmp_path)
    assert pod["spec"]["automountServiceAccountToken"] is False
    assert [v["name"] for v in pod["spec"]["volumes"]] == ["conf"]
    for c in pod["spec"]["containers"] + pod["spec"]["initContainers"]:
        assert [m["name"] for m in c.get("volumeMounts") or []] in (["conf"], []), c["name"]
    assert "api-token-needs-a-reason" not in failed_rules(pod_security, fixture, CNPG_SA, tmp_path)


@requires_kyverno
@requires_helm
def test_a_justified_pod_keeps_its_token(pod_security, tmp_path):
    """Taking the token from the CloudNativePG instance manager would stop it watching its
    own Cluster; the annotation is how that stays visible instead of hard-coded."""
    pod = mutated(pod_security, FIXTURES / "api-token-justified-pod.yaml", tmp_path)
    assert "automountServiceAccountToken" not in pod["spec"]
    failed = failed_rules(pod_security, FIXTURES / "api-token-justified-pod.yaml",
                          CNPG_SA, tmp_path)
    assert "api-token-needs-a-reason" not in failed


@requires_kyverno
@requires_helm
def test_a_pod_that_asks_for_a_token_without_a_reason_is_refused(pod_security, tmp_path):
    failed = failed_rules(pod_security, FIXTURES / "api-token-unjustified-pod.yaml",
                          CNPG_SA, tmp_path)
    assert "api-token-needs-a-reason" in failed


@requires_kyverno
@requires_helm
def test_a_projected_api_token_without_a_reason_is_refused(pod_security, tmp_path):
    """F-R2DEP3-002: automount off and the same token mounted as a projected volume."""
    failed = failed_rules(pod_security, FIXTURES / "api-token-projected-unjustified-pod.yaml",
                          CNPG_SA, tmp_path)
    assert "api-token-needs-a-reason" in failed


@requires_kyverno
@requires_helm
def test_a_projected_token_for_another_audience_needs_no_reason(pod_security, tmp_path):
    """A token bound to Keycloak's audience is refused by the API server: no API client."""
    failed = failed_rules(pod_security, FIXTURES / "keycloak-audience-token-pod.yaml",
                          CNPG_SA, tmp_path)
    assert "api-token-needs-a-reason" not in failed


@requires_kyverno
@requires_helm
def test_a_pod_outside_this_deployment_keeps_its_token(pod_security, tmp_path):
    """traefik, cert-manager and kyverno itself are API clients by trade. A cluster-wide
    token mutation would break them on their next restart."""
    pod = mutated(pod_security, FIXTURES / "unhardened-pod-other-namespace.yaml", tmp_path)
    assert "automountServiceAccountToken" not in pod["spec"]


HELM_TEST_HOOK = "helm.sh/hook"


def _pod_specs_of_jobs(docs: list[dict]):
    """Every Job and CronJob pod template the deployment renders, minus helm test hooks.

    Test hooks are excluded on the same grounds the repository's own Kyverno scan excludes
    them: `helmfile sync` never applies them, so no admission webhook ever judges them.
    """
    for doc in docs:
        kind = doc.get("kind")
        if kind not in ("Job", "CronJob"):
            continue
        hook = (doc["metadata"].get("annotations") or {}).get(HELM_TEST_HOOK, "")
        if "test" in hook:
            continue
        spec = doc["spec"]
        template = (
            spec["template"]
            if kind == "Job"
            else spec["jobTemplate"]["spec"]["template"]
        )
        yield doc["metadata"]["name"], template["spec"]


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_every_rendered_job_satisfies_the_policies_this_cluster_enforces(rendered, env):
    """The Pod Security policies are tested above against synthetic pods, and the render is
    tested elsewhere, but nothing put the two together — so a Job that the cluster would refuse
    rendered happily and only failed at `dev-apply`, inside a post-upgrade hook:

        admission webhook "validate.kyverno.svc-fail" denied the request:
        Job/dev/ckan-datastore-db-create-role-ckan-datastore-read-readonly was blocked
        drop-all-capabilities, require-ro-rootfs, require-run-as-nonroot

    A hook Job is the worst place to learn it: the release is already half-upgraded.
    """
    failures = []
    for name, spec in _pod_specs_of_jobs(rendered(env)):
        pod = spec.get("securityContext") or {}
        for container in spec.get("containers", []):
            ctx = container.get("securityContext") or {}
            where = f"{name}/{container['name']}"
            if not ctx.get("runAsNonRoot", pod.get("runAsNonRoot")):
                failures.append(f"{where}: runAsNonRoot is not true")
            if not ctx.get("readOnlyRootFilesystem"):
                failures.append(f"{where}: readOnlyRootFilesystem is not true")
            if (ctx.get("capabilities") or {}).get("drop") != ["ALL"]:
                failures.append(f"{where}: capabilities.drop is not [ALL]")
    assert not failures, f"{env} renders Jobs this cluster's Kyverno policies refuse:\n" + "\n".join(
        failures
    )


def api_audience_tokens(spec: dict) -> list[str]:
    """Volumes projecting a ServiceAccount token with no audience: the API server's own, the
    same API client token automount would mount (F-R2DEP3-002)."""
    return [
        volume["name"]
        for volume in spec.get("volumes") or []
        for source in (volume.get("projected") or {}).get("sources") or []
        if "serviceAccountToken" in source and not (source["serviceAccountToken"] or {}).get("audience")
    ]


@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_every_workload_that_mounts_an_api_token_justifies_it(rendered, env):
    """`justify-api-token-access` refuses a pod that mounts a ServiceAccount token without
    `security.joinedcontext.com/api-access-reason`. The policy is judged at admission, so a
    workload that forgets the annotation renders cleanly, passes CI, and then fails silently on
    the cluster: the ReplicaSet cannot create a pod, the Deployment sits at UP-TO-DATE 0, the old
    pod keeps serving, and `helm --wait` reports only `context deadline exceeded`.

    That is what T-0411 did to the Portal — the reason was written, but in a YAML comment rather
    than the annotation the policy reads.
    """
    failures = []
    for doc in rendered(env):
        if doc.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet"):
            continue
        template = doc["spec"]["template"]
        spec = template["spec"]
        if spec.get("automountServiceAccountToken") is not True and not api_audience_tokens(spec):
            continue
        reason = (template["metadata"].get("annotations") or {}).get(
            "security.joinedcontext.com/api-access-reason"
        )
        if not (reason or "").strip():
            failures.append(f"{doc['kind']}/{doc['metadata']['name']}")
    assert not failures, (
        f"{env}: these mount an API token with no security.joinedcontext.com/api-access-reason, "
        f"so Kyverno refuses their pods: {failures}"
    )


@requires_helm
def test_every_placeholder_is_substituted(policies):
    """A placeholder the chart never replaced is a rule comparing a real principal against a
    string nobody can be: Kyverno accepts the policy, the reports stay clean, and the control
    the rule names is never enforced (T-0815, OPS-30, CC-12)."""
    left = sorted(set(re.findall(r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+", policies.read_text())))
    assert left == [], f"the rendered policies still carry placeholders: {left}"


#: The rules T-2480 installs in Audit first, by the file name the chart's values use.
# The privileged, hostPath and host-namespace rules were promoted after dev reported them clean
# (T-2480, 2026-10-12); these two still have offenders on dev.
STAGED_BY_T2480 = {
    "require-image-checksum",
    "require-pod-requests-limits",
}


def staged_policy_names(staged: set) -> set:
    """The ClusterPolicy names of the staged files; a vendored file's policy may be named apart
    from its file (require-pod-requests-limits is `require-requests-limits`)."""
    return {
        yaml.safe_load(
            (CHART / "files/upstream" / f"{name}.yaml").read_text()
        )["metadata"]["name"]
        for name in staged
    }


def test_production_renders_every_runtime_policy_in_enforce(rendered):
    """OPS-29: in the production profile every validating rule refuses at admission. The action
    is read from the render itself, with no `--set`, so a default that slides back to Audit
    turns this red. Workload exemptions go through the opt-out annotation that
    `justify-linkerd-inject-opt-out` reads, so that policy has to be there too."""
    policies = [d for d in rendered("production") if d.get("kind") in ("ClusterPolicy", "Policy")]
    names = {p["metadata"]["name"] for p in policies}
    assert "justify-linkerd-inject-opt-out" in names, sorted(names)
    staged = set(yaml.safe_load((CHART / "values.yaml").read_text())["podSecurity"]["staged"])
    # T-2480: the owner's order is Audit, fix the offenders from dev's PolicyReports, then
    # Enforce. Only these five may report in production, each until it is promoted.
    assert staged <= STAGED_BY_T2480, f"a rule staged outside T-2480: {sorted(staged - STAGED_BY_T2480)}"
    staged_names = staged_policy_names(staged)
    audit = [
        f"{p['metadata']['name']}/{rule['name']}: {rule['validate'].get('failureAction')}"
        for p in policies
        for rule in p["spec"].get("rules", [])
        if "validate" in rule
        and p["metadata"]["name"] not in staged_names
        and (rule["validate"].get("failureAction") or p["spec"].get("validationFailureAction")) != "Enforce"
    ]
    assert not audit, "rules that only report in production:\n" + "\n".join(audit)


def test_production_admits_no_unmeshed_pod_and_its_edge_takes_only_mesh_traffic(rendered):
    """OPS-39: plaintext between components stays inside the Linkerd trust boundary. A pod in a
    meshed namespace without the proxy, or a meshed namespace without a default inbound policy,
    is refused at admission; the edge data plane accepts only mTLS-authenticated clients. The
    ACME HTTP-01 solver is the one unauthenticated server, because Let's Encrypt reaches it in
    plaintext from outside the cluster by design."""
    docs = rendered("production")
    policies = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "ClusterPolicy"}
    for name in ("require-linkerd-sidecar", "require-meshed-namespace-inbound-policy"):
        assert name in policies, f"{name} is not rendered in production"
        actions = {r["validate"].get("failureAction") for r in policies[name]["spec"]["rules"]}
        assert actions == {"Enforce"}, f"{name}: {actions}"
    servers = {d["metadata"]["name"]: d["spec"] for d in docs if d.get("kind") == "Server"}
    open_servers = sorted(n for n, s in servers.items() if s.get("accessPolicy") == "all-unauthenticated")
    assert open_servers == ["apisix-configuration-acme-http01-solver"], open_servers
    assert servers["apisix-configuration-apisix-gateway"]["accessPolicy"] == "all-authenticated"


def test_the_linkerd_cni_plugin_is_given_its_opt_out_reason_and_nothing_else_is(rendered):
    """T-2832: the linkerd2-cni chart opts its DaemonSet out of injection with no way to add an
    annotation, and justify-linkerd-inject-opt-out refused it at admission on dev. The reason is
    written for the plugin's own namespace only: every other opt-out still has to state its own."""
    docs = rendered("production")
    policy = next(d for d in docs if d.get("kind") == "ClusterPolicy"
                  and d["metadata"]["name"] == "explain-linkerd-cni-opt-out")
    rules = policy["spec"]["rules"]
    assert {r["name"] for r in rules} == {"linkerd-cni-daemonset-reason", "linkerd-cni-pod-reason"}
    for rule in rules:
        for match in rule["match"]["any"]:
            assert match["resources"]["namespaces"] == ["linkerd-cni"], rule["name"]
        assert "validate" not in rule, "it states a reason; it refuses nothing and exempts nothing else"
    daemonset, pod = (next(r for r in rules if r["name"] == name)["mutate"]["patchStrategicMerge"]
                      for name in ("linkerd-cni-daemonset-reason", "linkerd-cni-pod-reason"))
    reason = daemonset["spec"]["template"]["metadata"]["annotations"]["mesh.joinedcontext.com/opt-out-reason"]
    assert reason.strip() and pod["metadata"]["annotations"]["mesh.joinedcontext.com/opt-out-reason"] == reason


@requires_helm
def test_a_staged_rule_reports_in_audit_whatever_the_environment_enforces(pod_security):
    """T-2480, OPS-29: the chart renders enforcing here (`failureAction=Enforce`); the five new
    rules still only report, and the four promoted ones refuse. Both stop at the deployment's
    namespaces."""
    docs = {d["metadata"]["name"]: d for d in yaml.safe_load_all(pod_security.read_text()) if d}
    staged = staged_policy_names(STAGED_BY_T2480)
    assert staged <= set(docs), sorted(staged - set(docs))
    for name, policy in docs.items():
        for rule in policy["spec"].get("rules", []):
            if "validate" not in rule or name not in staged | {
                "drop-all-capabilities", "require-ro-rootfs",
                "require-run-as-non-root-user", "require-run-as-nonroot",
            }:
                continue
            want = "Audit" if name in staged else "Enforce"
            assert rule["validate"]["failureAction"] == want, f"{name}/{rule['name']}"
            for matcher in rule["match"]["any"]:
                assert matcher["resources"]["namespaces"] == ["dev"], f"{name}/{rule['name']}"


@requires_helm
def test_a_rule_both_staged_and_promoted_fails_the_render():
    """A name in both lists would render twice with two actions; the render refuses it."""
    result = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "linkerd.enabled=false", "--set", "operators.enabled=false",
         "--set", "podSecurity.namespaces={dev}",
         "--set", "podSecurity.policies={require-image-checksum}", "--namespace", "dev"],
        capture_output=True, text=True, check=False,
    )
    assert result.returncode != 0
    assert "both staged and promoted" in result.stderr


@requires_helm
def test_the_platform_images_are_verified_against_their_workflow_identity(pod_security):
    """T-2480: our own images carry the keyless signature reusable-container-sign.yml makes;
    admission checks it for our registry path only, in Audit while staged, and never holds a
    pod when Rekor is out of reach."""
    docs = {d["metadata"]["name"]: d for d in yaml.safe_load_all(pod_security.read_text()) if d}
    policy = docs["verify-platform-images"]["spec"]
    assert policy["failurePolicy"] == "Ignore"
    [rule] = policy["rules"]
    assert rule["match"]["any"][0]["resources"]["namespaces"] == ["dev"]
    [verify] = rule["verifyImages"]
    assert verify["imageReferences"] == ["ghcr.io/marek-mraz-jc/*"]
    assert verify["failureAction"] == "Audit"
    keyless = verify["attestors"][0]["entries"][0]["keyless"]
    assert keyless["issuer"] == "https://token.actions.githubusercontent.com"
    assert keyless["subjectRegExp"] == "^https://github.com/marek-mraz-jc/"
    assert "verify-production-images" not in docs  # no production namespace, no policy


def production_images(tmp_path, *extra):
    out = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "linkerd.enabled=false", "--set", "operators.enabled=false",
         "--set", "podSecurity.namespaces={dev}",
         "--set", "imageSignatures.production.namespaces={jc-prod,jc-prod-mqtt}", *extra, "--namespace", "dev"],
        capture_output=True, text=True,
    )
    return out


@requires_helm
def test_production_namespaces_admit_only_images_the_main_image_workflows_signed(tmp_path):
    """T-6719, G-1: jc-prod's pods admit our images only when image.yml (or the embedder's
    image-embedder.yml) on main signed them; dev's rule keeps admitting any workflow of the org."""
    result = production_images(tmp_path)
    assert result.returncode == 0, result.stderr
    docs = {d["metadata"]["name"]: d for d in yaml.safe_load_all(result.stdout) if d}
    policy = docs["verify-production-images"]["spec"]
    [rule] = policy["rules"]
    assert rule["match"]["any"][0]["resources"]["namespaces"] == ["jc-prod", "jc-prod-mqtt"]
    [verify] = rule["verifyImages"]
    assert (verify["imageReferences"], verify["required"], verify["verifyDigest"]) == (["ghcr.io/marek-mraz-jc/*"], True, True)
    subject = re.compile(verify["attestors"][0]["entries"][0]["keyless"]["subjectRegExp"])
    # The identities jc-prod's images carry (cosign verify, 2026-11-11), and what must not pass.
    for signed in ("https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image.yml@refs/heads/main",
                   "https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image-embedder.yml@refs/heads/main",
                   "https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image-apps.yml@refs/heads/main",
                   "https://github.com/marek-mraz-jc/joinedcontext-platform/.github/workflows/image.yml@refs/heads/main"):
        assert subject.match(signed), signed
    for forged in ("https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image.yml@refs/heads/jc-portal-1",
                   "https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/ci.yml@refs/heads/main",
                   "https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image.yml@refs/heads/main-x",
                   "https://github.com/marek-mraz-jcx/joinedcontext/.github/workflows/image.yml@refs/heads/main",
                   "https://github.com/marek-mraz-jc/joinedcontext/.github/workflows/image.yml@refs/tags/v1"):
        assert not subject.match(forged), forged
    # Audit while staged never holds a pod; Enforce fails closed.
    assert (verify["failureAction"], policy["failurePolicy"]) == ("Audit", "Ignore")
    enforced = production_images(tmp_path, "--set", "imageSignatures.production.failureAction=Enforce")
    spec = {d["metadata"]["name"]: d for d in yaml.safe_load_all(enforced.stdout) if d}["verify-production-images"]["spec"]
    assert (spec["rules"][0]["verifyImages"][0]["failureAction"], spec["failurePolicy"]) == ("Enforce", "Fail")


def test_dev_enforces_the_signature_rule_on_the_production_namespaces(rendered):
    # T-6719: after one audit pass the rule refuses, and the webhook fails closed.
    policy = next(d for d in rendered("dev") if d.get("kind") == "ClusterPolicy"
                  and d["metadata"]["name"] == "verify-production-images")["spec"]
    assert policy["rules"][0]["verifyImages"][0]["failureAction"] == "Enforce"
    assert policy["failurePolicy"] == "Fail"


@requires_helm
def test_a_production_subject_on_any_branch_but_main_refuses_to_render(tmp_path):
    result = production_images(tmp_path, "--set-string",
                               "imageSignatures.production.subjectRegExp=^https://github\\.com/marek-mraz-jc/.*")
    assert result.returncode != 0 and "subjectRegExp" in result.stderr, result.stderr



@pytest.fixture(scope="module")
def production_tier(tmp_path_factory):
    """The chart with the production tier switched on, as the first labelled environment will."""
    out = tmp_path_factory.mktemp("production-tier") / "policies.yaml"
    out.write_text(subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "failureAction=Enforce", "--set", "linkerd.enabled=false",
         "--set", "operators.enabled=false", "--set", "podSecurity.namespaces={dev}",
         "--set", "imageSignatures.productionTier.enabled=true", "--namespace", "dev"],
        capture_output=True, text=True, check=True,
    ).stdout)
    return out


@requires_helm
def test_the_production_tier_is_off_until_an_environment_turns_it_on(pod_security):
    """T-4721: failing closed in front of every Pod with no production namespace yet would let a
    Kyverno outage hold the whole node, so the default renders no such policy."""
    names = {d["metadata"]["name"] for d in yaml.safe_load_all(pod_security.read_text()) if d}
    assert "require-full-tests-in-production" not in names
    assert "verify-platform-images" in names


@requires_helm
def test_a_production_tier_namespace_admits_only_fully_tested_images(production_tier):
    """T-4721: a namespace labelled tier=production admits our images only with promote.yml's
    full-tests attestation, enforced and failing closed; a dev-<N> digest has none. Selected by
    the label alone, so no namespace is judged until one carries it."""
    docs = {d["metadata"]["name"]: d for d in yaml.safe_load_all(production_tier.read_text()) if d}
    policy = docs["require-full-tests-in-production"]["spec"]
    assert policy["failurePolicy"] == "Fail"
    [rule] = policy["rules"]
    [match] = rule["match"]["any"]
    assert match["resources"]["namespaceSelector"] == {"matchLabels": {"joinedcontext.com/tier": "production"}}
    assert "namespaces" not in match["resources"]
    [verify] = rule["verifyImages"]
    assert verify["imageReferences"] == ["ghcr.io/marek-mraz-jc/*"]
    assert verify["failureAction"] == "Enforce" and verify["required"] is True
    [attestation] = verify["attestations"]
    assert attestation["type"] == "https://joinedcontext.com/attestations/full-tests/v1"
    subject = re.compile(attestation["attestors"][0]["entries"][0]["keyless"]["subjectRegExp"])
    promote = "https://github.com/marek-mraz-jc/joinedcontext-platform/.github/workflows/promote.yml@refs/heads/main"
    assert subject.search(promote)
    # ship's own signature is not the full-tests one, nor is a promote.yml run off main
    assert not subject.search(promote.replace("promote.yml", "image.yml"))
    assert not subject.search(promote.replace("refs/heads/main", "refs/heads/agent/x"))
    broker = "https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker/.github/workflows/image-jc.yml@refs/heads/dev"
    assert subject.search(broker)
    assert not subject.search(broker.replace("image-jc.yml", "ship.yml"))

BAD_PODS = {
    # T-2480: one pod per step of the attack, each refused by its own rule.
    "privileged": ({"securityContext": {"privileged": True}}, {}, "privileged-containers"),
    "host-path": ({}, {"volumes": [{"name": "h", "hostPath": {"path": "/"}}]}, "host-path"),
    "host-network": ({}, {"hostNetwork": True}, "host-namespaces"),
    "tag-only": ({"image": "nginx:1.27"}, {}, "require-image-checksum"),
    "no-limits": ({"resources": None}, {}, "validate-resources"),
}


@requires_kyverno
@requires_helm
@pytest.mark.parametrize("step", sorted(BAD_PODS))
def test_each_step_of_the_admission_attack_is_reported_by_its_rule(pod_security, tmp_path, step):
    """T-2480, T-1711, OPS-29: the hardened pod with one thing wrong fails exactly the rule for
    that thing (reported in Audit while staged, refused once promoted)."""
    container_change, spec_change, rule = BAD_PODS[step]
    pod = yaml.safe_load((FIXTURES / "hardened-pod.yaml").read_text())
    container = pod["spec"]["containers"][0]
    for key, value in container_change.items():
        if key == "securityContext":
            container[key].update(value)
        elif value is None:
            container.pop(key, None)
        else:
            container[key] = value
    pod["spec"].update(spec_change)
    resource = tmp_path / f"{step}.yaml"
    resource.write_text(yaml.safe_dump(pod))
    failed = failed_rules(pod_security, resource, CNPG_SA, tmp_path)
    assert rule in failed, f"{step}: {sorted(failed)}"


@pytest.mark.xdist_group("deployment-environments-testing")
@pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")
@pytest.mark.parametrize("action", ["", "Warn"])
def test_an_environment_without_an_action_fails_the_render_instead_of_auditing(action):
    """T-2480: the helmfile used to fall back to Audit when `global.runtimePolicies.failureAction`
    was missing, so a typo turned enforcement off without a word. Now the render stops."""
    env_dir = PROJECT_ROOT / "deployment/environments/testing"
    shutil.rmtree(env_dir, ignore_errors=True)
    env_dir.mkdir(parents=True)
    (env_dir / "global.yaml.gotmpl").write_text(
        "global:\n  instanceSlug: dev\n  runtimePolicies:\n"
        f"    enabled: true\n    failureAction: {action!r}\n"
    )
    try:
        result = subprocess.run(
            ["helmfile", "template", "-f", "helmfile.yaml", "--skip-deps", "-e", "testing"],
            cwd=PROJECT_ROOT / "deployment", capture_output=True, text=True, check=False,
        )
    finally:
        shutil.rmtree(env_dir, ignore_errors=True)
    assert result.returncode != 0
    assert "failureAction must be Audit or Enforce" in result.stderr + result.stdout


# Online: kyverno fetches the real signature and attestations from ghcr.io and Rekor, so it runs
# only when asked (JC_KYVERNO_ONLINE=1), never as a flaky fast-lane gate.
requires_online = pytest.mark.skipif(os.environ.get("JC_KYVERNO_ONLINE") != "1", reason="set JC_KYVERNO_ONLINE=1")
SHIPPED = ("ghcr.io/marek-mraz-jc/joinedcontext-platform"
           "@sha256:2f75206823a576a2e9f958311063daff0d9c61a7d3b51920612c00051927f835")  # signed, never promoted


@requires_online
@requires_kyverno
@requires_helm
@pytest.mark.parametrize("tier, refused", [("production", True), ("demo", False)])
def test_a_signed_dev_image_is_refused_only_in_a_production_tier_namespace(production_tier, tmp_path, tier, refused):
    """T-4721, against the registry: a digest ship signed but promote never attested starts in a
    demo-tier namespace and is refused in a production-tier one."""
    pod = tmp_path / "pod.yaml"
    pod.write_text(yaml.safe_dump({"apiVersion": "v1", "kind": "Pod", "metadata": {"name": "gw", "namespace": "ns"},
                                   "spec": {"containers": [{"name": "gw", "image": SHIPPED}]}}))
    values = tmp_path / "values.yaml"
    values.write_text(yaml.safe_dump({"apiVersion": "cli.kyverno.io/v1alpha1", "kind": "Values",
                                      "namespaceSelector": [{"name": "ns", "labels": {"joinedcontext.com/tier": tier}}]}))
    docs = [d for d in yaml.safe_load_all(production_tier.read_text())
            if d and d["metadata"]["name"] in ("verify-platform-images", "require-full-tests-in-production")]
    policy = tmp_path / "policy.yaml"
    policy.write_text(yaml.safe_dump_all(docs))
    out = subprocess.run(["kyverno", "apply", str(policy), "--resource", str(pod), "--values-file", str(values)],
                         capture_output=True, text=True, timeout=120).stdout
    assert ("require-full-tests-in-production -> resource ns/Pod/gw failed" in out) is refused, out
    assert "error: 0" in out, out


def plural(kind: str) -> str:
    kind = kind.lower()
    if kind.endswith("y"):
        return kind[:-1] + "ies"
    return kind + ("es" if kind.endswith("s") else "s")


@requires_helm
def test_the_drift_reader_reads_each_rendered_kind_and_nothing_beyond_objects():
    # T-4956 review: resources '*' also grants nodes/proxy (kubelet exec over GET), services/proxy
    # and pods/log. The role names each kind the environments render, get and list only.
    rendered = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--namespace", "dev",
         "--set", "driftReader.secretNamespaces={dev,dev-portal}"],
        capture_output=True, text=True, check=True,
    ).stdout
    docs = [d for d in yaml.safe_load_all(rendered) if d]
    role = next(d for d in docs if d["kind"] == "ClusterRole" and d["metadata"]["name"] == "jc-drift-reader")
    # T-5702 (F-DEP2-001): Secrets only in the release namespaces, never cluster-wide.
    secret_roles = {d["metadata"]["namespace"]: d for d in docs
                    if d["kind"] == "Role" and d["metadata"]["name"] == "jc-drift-reader-secrets"}
    assert set(secret_roles) == {"dev", "dev-portal"}
    for ns, secret_role in secret_roles.items():
        assert secret_role["rules"] == [{"apiGroups": [""], "resources": ["secrets"], "verbs": ["get", "list"]}]
        binding = next(d for d in docs if d["kind"] == "RoleBinding" and d["metadata"]["namespace"] == ns)
        assert binding["roleRef"]["kind"] == "Role" and binding["subjects"][0]["name"] == "jc-drift-reader"
    granted = {("", "secrets")}
    for rule in role["rules"]:
        assert rule["verbs"] == ["get", "list"], rule
        assert "*" not in rule["apiGroups"] and "*" not in rule["resources"], rule
        assert "resourceNames" not in rule and "nonResourceURLs" not in rule, rule
        for resource in rule["resources"]:
            assert resource != "secrets", "cluster-wide Secrets read every credential of every namespace"
            assert "/" not in resource, f"subresource {resource} (proxy, exec, attach, portforward, log)"
            granted |= {(group, resource) for group in rule["apiGroups"]}
    rendered_kinds = set()
    for golden in (PROJECT_ROOT / "ci/golden").glob("*.txt"):
        for line in golden.read_text().splitlines():
            api_version, _, kind = line.split()[0].rpartition("/")
            group = api_version.rpartition("/")[0] if "/" in api_version else ""
            rendered_kinds.add((group, plural(kind)))
    assert rendered_kinds, "no golden render to compare with"
    assert rendered_kinds - granted == set(), "kinds helm diff must read"
    assert granted - rendered_kinds == set(), "grants no environment renders"


@requires_kyverno
@requires_helm
def test_a_meshed_namespace_must_name_an_authenticated_inbound_policy(tmp_path):
    """T-5702 (F-DEP2-009): the annotation used to pass with any value, all-unauthenticated
    included, the very state the policy exists to prevent."""
    rendered = subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "driftReader.enabled=false",
         "--set", "failureAction=Enforce", "--set", "operators.enabled=false", "--namespace", "dev"],
        capture_output=True, text=True, check=True,
    ).stdout
    policy = tmp_path / "policy.yaml"
    policy.write_text(yaml.safe_dump(next(
        d for d in yaml.safe_load_all(rendered)
        if d and d["metadata"]["name"] == "require-meshed-namespace-inbound-policy")))

    def refused(inbound):
        annotations = {"linkerd.io/inject": "enabled"}
        if inbound is not None:
            annotations["config.linkerd.io/default-inbound-policy"] = inbound
        resource = tmp_path / "namespace.yaml"
        resource.write_text(yaml.safe_dump({"apiVersion": "v1", "kind": "Namespace",
                                            "metadata": {"name": "dev-x", "annotations": annotations}}))
        return "inbound-policy-required" in failed_rules(policy, resource, "kubernetes-admin", tmp_path)

    for value in ("cluster-authenticated", "all-authenticated", "deny"):
        assert not refused(value), value
    for value in (None, "", "all-unauthenticated", "cluster-unauthenticated", "audit"):
        assert refused(value), value


def test_runtime_namespaces_get_the_pod_rules_and_the_drift_reader_only_release_secrets(rendered):
    """T-5702 (F-DEP2-003, F-DEP2-001): App, preview and demo namespaces exist only at runtime,
    so no component names them; the Pod Security and signature policies match them by wildcard.
    The drift reader reads Secrets in the release namespaces alone."""
    docs = rendered("dev")
    policies = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "ClusterPolicy"}
    for name in ("require-run-as-nonroot", "require-ro-rootfs", "verify-platform-images"):
        assert name in policies, name
        for rule in policies[name]["spec"]["rules"]:
            for matcher in rule["match"]["any"]:
                assert {"dev-*-apps", "pv-*", "demo-*"} <= set(matcher["resources"]["namespaces"]), (name, rule["name"])
    secret_roles = {d["metadata"]["namespace"] for d in docs
                    if d.get("kind") == "Role" and d["metadata"]["name"] == "jc-drift-reader-secrets"}
    portal = next(d["metadata"]["namespace"] for d in docs
                  if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    assert portal in secret_roles
    assert not {"kube-system", "default"} & secret_roles
    assert not any(ns.endswith("-apps") or "*" in ns for ns in secret_roles), secret_roles


def test_the_agent_reader_holds_view_and_node_reads_only():
    """T-6546: workers read dev and stage as agent-reader. It may read what `view` reads plus
    nodes and their metrics, and nothing else: no Secret, no write verb, no subresource such as
    pods/exec or nodes/proxy, and no stored token. Off unless the environment turns it on."""
    off = subprocess.run(["helm", "template", "runtime-policies", str(CHART)],
                         check=True, capture_output=True, text=True).stdout
    assert "agent-reader" not in off
    docs = [d for d in yaml.safe_load_all(subprocess.run(
        ["helm", "template", "runtime-policies", str(CHART), "--set", "agentReader.enabled=true"],
        check=True, capture_output=True, text=True).stdout) if d]
    mine = [d for d in docs if "agent-reader" in d["metadata"]["name"]]
    assert sorted(d["kind"] for d in mine) == ["ClusterRole", "ClusterRoleBinding", "ClusterRoleBinding", "ServiceAccount"]
    assert not any(d["kind"] == "Secret" for d in mine)
    bindings = {d["roleRef"]["name"] for d in mine if d["kind"] == "ClusterRoleBinding"}
    assert bindings == {"view", "jc-agent-reader-nodes"}
    role = next(d for d in mine if d["kind"] == "ClusterRole")
    for rule in role["rules"]:
        assert set(rule["verbs"]) <= {"get", "list"}, rule
        assert not any("/" in r or r == "*" or r == "secrets" for r in rule["resources"]), rule
