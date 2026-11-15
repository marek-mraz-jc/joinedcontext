"""The pipelines' token service (PL-19, T-1508, joinedcontext-docs Architecture/12 §3).

One pod beside the runner mints each pipeline's token from its own Kubernetes ServiceAccount.
These read the rendered manifests: off by default, and when on, the service holds one verb on
one subresource, reaches the Kubernetes API, and admits the runner alone, while the runner
itself still reaches no Kubernetes API.
"""

import pytest

pytestmark = pytest.mark.xdist_group("render-pipeline-tokens")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind}/{name}: {len(found)}"
    return found[0]


def ports(rule):
    return {p["port"] for p in rule.get("ports", [])}


def test_the_service_is_off_by_default_and_the_runner_still_reaches_no_kubernetes_api(rendered):
    docs = rendered("local")
    assert not [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-tokens"]
    runner = one(docs, "NetworkPolicy", "pipeline-runner")["spec"]
    assert all(6443 not in ports(rule) for rule in runner["egress"]), "the runner never reaches the Kubernetes API"
    tokens_rule = [rule for rule in runner["egress"] if 4180 in ports(rule)]
    assert len(tokens_rule) == 1, "the runner reaches the token service on 4180"
    env = {e["name"]: e.get("value") for c in one(docs, "Deployment", "pipeline-runner")["spec"]["template"]["spec"]["containers"]
           for e in c.get("env", [])}
    assert env["JC_PIPELINE_TOKEN_URL"].startswith("http://pipeline-tokens.") and env["JC_PIPELINE_TOKEN_URL"].endswith(":4180/token")


def test_the_token_services_policy_admits_the_runner_alone_and_reaches_the_api(rendered):
    policy = one(rendered("local"), "NetworkPolicy", "pipeline-tokens")["spec"]
    (ingress,) = policy["ingress"]
    assert ports(ingress) == {4180}
    assert [peer["podSelector"]["matchLabels"] for peer in ingress["from"]] == [
        {"app.kubernetes.io/name": "pipeline-runner-runner"}]
    assert all("namespaceSelector" in peer for peer in ingress["from"])
    assert any(6443 in ports(rule) for rule in policy["egress"])


def enable_tokens(tree):
    path = tree / "components/pipeline-runner/default-environment.yaml.gotmpl"
    text = path.read_text()
    for part in ("tokens", "identities"):
        assert f"  {part}:\n    enabled: false\n" in text, part
        text = text.replace(f"  {part}:\n    enabled: false\n", f"  {part}:\n    enabled: true\n")
    path.write_text(text)


def test_switched_on_the_service_mints_only_in_the_pipelines_namespace_with_its_own_token(rendered_variant):
    docs = rendered_variant("dev", enable_tokens)
    deployment_doc = one(docs, "Deployment", "pipeline-tokens")
    tokens_namespace = deployment_doc["metadata"]["namespace"]
    deployment = deployment_doc["spec"]["template"]["spec"]
    (container,) = [c for c in deployment["containers"] if c["name"] != "linkerd-proxy"]
    assert container["command"] == ["/usr/local/bin/jc-token-sidecar"]
    assert deployment["automountServiceAccountToken"] is False
    mounts = {m["mountPath"]: m for m in container["volumeMounts"]}
    assert mounts["/var/run/secrets/kubernetes.io/serviceaccount"]["readOnly"] is True
    env = {e["name"]: e.get("value") for e in container["env"]}
    assert env["JC_SIDECAR_LISTEN"] == "0.0.0.0:4180", "the service is called across the pod network"
    assert env["JC_TOKEN_URL"].startswith(env["JC_TOKEN_AUDIENCE"] + "/protocol/openid-connect/token")

    # The one grant: create serviceaccounts/token in the pipelines' own namespace, which is not
    # the namespace of the runner or of the service, so no workload's ServiceAccount is in reach.
    identities = env["JC_PIPELINE_NAMESPACE"]
    runner_namespace = one(docs, "Deployment", "pipeline-runner")["metadata"]["namespace"]
    assert identities not in {tokens_namespace, runner_namespace}
    roles = [d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "pipeline-tokens"]
    assert [r["metadata"]["namespace"] for r in roles] == [identities]
    assert roles[0]["rules"] == [{"apiGroups": [""], "resources": ["serviceaccounts/token"], "verbs": ["create"]}]
    binding = one(docs, "RoleBinding", "pipeline-tokens")
    assert binding["metadata"]["namespace"] == identities
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "pipeline-tokens", "namespace": tokens_namespace}]


def without_dev_switch(tree):
    (tree / "deployment/environments/_layers/hosted/pipeline-runner.yaml.gotmpl").unlink()


def test_switched_on_the_portal_keeps_the_pipelines_accounts_there_and_nothing_else(rendered_variant):
    def accounts_only(tree):
        without_dev_switch(tree)
        enable_tokens(tree)

    docs = rendered_variant("dev", accounts_only)
    identities = one(docs, "Role", "pipeline-tokens")["metadata"]["namespace"]
    portal = one(docs, "Deployment", "portal")
    env = {e["name"]: e.get("value") for c in portal["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_PORTAL_PIPELINE_NAMESPACE"] == identities
    assert env["JC_PORTAL_PIPELINE_IDENTITY"] == "project", "making the accounts does not switch the streams"
    role = one(docs, "Role", "portal-pipeline-accounts")
    assert role["metadata"]["namespace"] == identities
    assert role["rules"] == [{"apiGroups": [""], "resources": ["serviceaccounts"],
                              "verbs": ["get", "list", "create", "patch", "delete"]}]
    binding = one(docs, "RoleBinding", "portal-pipeline-accounts")
    assert binding["metadata"]["namespace"] == identities
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "portal",
                                    "namespace": portal["metadata"]["namespace"]}]


def test_off_the_portal_has_no_pipeline_namespace_and_no_grant(rendered):
    docs = rendered("local")
    assert not [d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "portal-pipeline-accounts"]
    portal = one(docs, "Deployment", "portal")
    names = {e["name"] for c in portal["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert "JC_PORTAL_PIPELINE_NAMESPACE" not in names


def test_dev_streams_present_their_own_pipeline_and_the_runner_resolves_no_secret_for_them(rendered):
    """T-4044: on dev every stream takes its token from the token service, so the Portal renders
    no client secret into one and the runner's streams API has none to hand out."""
    docs = rendered("dev")
    one(docs, "Deployment", "pipeline-tokens")
    identities = one(docs, "Role", "pipeline-tokens")["metadata"]["namespace"]
    portal = one(docs, "Deployment", "portal")
    env = {e["name"]: e.get("value") for c in portal["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_PORTAL_PIPELINE_IDENTITY"] == "pipeline"
    assert env["JC_PORTAL_PIPELINE_NAMESPACE"] == identities


def test_on_dev_the_runner_reads_no_client_secret(rendered):
    """T-4050: with the streams and the pipeline-test harness on the token service, the runner
    carries no `JC_CLIENT_SECRET*` at all, so no `{project}-pipelines` secret is read by a
    workload and the owner can retire them. Without the switch it still carries them."""
    def secret_env(docs):
        runner = one(docs, "Deployment", "pipeline-runner")
        return sorted(
            e["name"]
            for c in runner["spec"]["template"]["spec"]["containers"]
            for e in c.get("env", [])
            if e["name"].startswith("JC_CLIENT_SECRET") or "secretKeyRef" in e.get("valueFrom", {})
        )

    assert secret_env(rendered("dev")) == []
    assert "JC_CLIENT_SECRET_HELSINKI" in secret_env(rendered("local"))


@pytest.mark.parametrize("environment", ["local", "production"])
def test_elsewhere_the_streams_stay_on_the_project_client_until_switched(rendered, environment):
    portal = one(rendered(environment), "Deployment", "portal")
    env = {e["name"]: e.get("value") for c in portal["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_PORTAL_PIPELINE_IDENTITY"] == "project"


def test_the_streams_switch_alone_switches_nothing(rendered_variant):
    """`identities.streams` without the token service would send every stream to a service that
    is not there: the Portal keeps the project client."""
    def streams_only(tree):
        path = tree / "deployment/environments/_layers/hosted/pipeline-runner.yaml.gotmpl"
        path.write_text("---\npipeline-runner:\n  identities:\n    streams: true\n")

    portal = one(rendered_variant("dev", streams_only), "Deployment", "portal")
    env = {e["name"]: e.get("value") for c in portal["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_PORTAL_PIPELINE_IDENTITY"] == "project"
