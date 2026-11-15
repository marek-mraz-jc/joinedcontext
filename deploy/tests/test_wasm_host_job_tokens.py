"""The token service of the wasm Apps' jobs (AP-159, T-3372, joinedcontext-docs Architecture/12 §3).

A job runs as its App's job principal: the shard asks the service beside it, which mints a token
of the App's own Kubernetes ServiceAccount. These read the rendered manifests: off by default, and
when on, the service holds one verb on one subresource in the job identities' namespace, reaches
the Kubernetes API, and admits the shards alone.
"""

import pytest

pytestmark = pytest.mark.xdist_group("render-wasm-host-job-tokens")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind}/{name}: {len(found)}"
    return found[0]


def ports(rule):
    return {p["port"] for p in rule.get("ports", [])}


def env_of(deployment):
    return {e["name"]: e.get("value") for c in deployment["spec"]["template"]["spec"]["containers"]
            for e in c.get("env", [])}


def enable(tree):
    path = tree / "components/wasm-host/default-environment.yaml.gotmpl"
    text = path.read_text()
    for part in ("tokens", "identities"):
        assert f"  {part}:\n    enabled: false\n" in text, part
        text = text.replace(f"  {part}:\n    enabled: false\n", f"  {part}:\n    enabled: true\n")
    path.write_text(text)


def without_dev_switch(tree):
    (tree / "deployment/environments/_layers/hosted/wasm-host.yaml.gotmpl").unlink()


def test_off_by_default_no_service_no_namespace_and_no_token_url(rendered_variant):
    docs = rendered_variant("dev", without_dev_switch)
    assert not [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "wasm-host-tokens"]
    assert not [d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "wasm-host-tokens"]
    assert "JC_WASM_JOB_TOKEN_URL" not in env_of(one(docs, "Deployment", "jc-wasm-host-0"))
    assert "JC_PORTAL_APP_IDENTITY_NAMESPACE" not in env_of(one(docs, "Deployment", "portal"))


def test_dev_switches_it_on_and_the_portal_learns_the_namespace(rendered):
    # dev runs the federated mechanism (T-2868), so the jobs' principals are on there (T-3539).
    docs = rendered("dev")
    one(docs, "Deployment", "wasm-host-tokens")
    namespace = env_of(one(docs, "Deployment", "portal"))["JC_PORTAL_APP_IDENTITY_NAMESPACE"]
    assert namespace == "dev-app-identities"
    assert env_of(one(docs, "Deployment", "jc-wasm-host-0"))["JC_WASM_JOB_TOKEN_URL"]


def test_the_service_admits_the_shards_alone_and_the_shards_reach_it(rendered):
    docs = rendered("dev")
    policy = one(docs, "NetworkPolicy", "wasm-host-tokens")["spec"]
    (ingress,) = policy["ingress"]
    assert ports(ingress) == {4180}
    assert [peer["podSelector"]["matchLabels"] for peer in ingress["from"]] == [
        {"app.kubernetes.io/name": "wasm-host-shard"}]
    assert any(6443 in ports(rule) for rule in policy["egress"])
    shard = one(docs, "NetworkPolicy", "wasm-host-shard")["spec"]
    assert any(4180 in ports(rule) for rule in shard["egress"])
    assert all(6443 not in ports(rule) for rule in shard["egress"]), "a shard never reaches the Kubernetes API"


def test_switched_on_the_service_mints_only_in_the_app_identities_namespace(rendered_variant):
    docs = rendered_variant("dev", enable)
    deployment = one(docs, "Deployment", "wasm-host-tokens")
    tokens_namespace = deployment["metadata"]["namespace"]
    pod = deployment["spec"]["template"]["spec"]
    (container,) = [c for c in pod["containers"] if c["name"] != "linkerd-proxy"]
    assert container["command"] == ["/usr/local/bin/jc-token-sidecar"]
    assert pod["automountServiceAccountToken"] is False
    env = {e["name"]: e.get("value") for e in container["env"]}
    assert env["JC_SIDECAR_IDENTITY"] == "appjob"
    assert "JC_PIPELINE_NAMESPACE" not in env
    identities = env["JC_APP_IDENTITY_NAMESPACE"]
    assert identities.endswith("-app-identities") and identities != tokens_namespace

    namespace = one(docs, "Namespace", identities)
    assert namespace["metadata"]["labels"]["joinedcontext.com/holds"] == "app-job-serviceaccounts"
    role = one(docs, "Role", "wasm-host-tokens")
    assert role["metadata"]["namespace"] == identities
    assert role["rules"] == [{"apiGroups": [""], "resources": ["serviceaccounts/token"], "verbs": ["create"]}]
    binding = one(docs, "RoleBinding", "wasm-host-tokens")
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "wasm-host-tokens", "namespace": tokens_namespace}]
    # The pipelines' grant is untouched by the shared chart.
    assert not [d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "pipeline-tokens"
                and d["metadata"]["namespace"] == identities]

    shard_env = env_of(one(docs, "Deployment", "jc-wasm-host-0"))
    assert shard_env["JC_WASM_JOB_TOKEN_URL"] == f"http://wasm-host-tokens.{tokens_namespace}.svc.cluster.local:4180/token"

    portal = one(docs, "Deployment", "portal")
    assert env_of(portal)["JC_PORTAL_APP_IDENTITY_NAMESPACE"] == identities
    accounts = one(docs, "Role", "portal-app-job-accounts")
    assert accounts["metadata"]["namespace"] == identities
    assert accounts["rules"] == [{"apiGroups": [""], "resources": ["serviceaccounts"],
                                  "verbs": ["get", "list", "create", "patch", "delete"]}]


def test_every_shard_may_ask_for_a_token(rendered_variant):
    """Each shard runs its own Apps' jobs, so the service admits every shard's pods, in the plain
    policy and in the mesh one; a rule naming shard-0 alone left shard-1's jobs without a token."""
    docs = rendered_variant("dev", enable)
    shards = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"].startswith("jc-wasm-host-")]
    assert len(shards) == 2, [d["metadata"]["name"] for d in shards]
    for name in ("wasm-host-tokens", "wasm-host-tokens-linkerd-in"):
        policy = one(docs, "NetworkPolicy", name)
        (ingress,) = policy["spec"]["ingress"]
        for shard in shards:
            labels = shard["spec"]["template"]["metadata"]["labels"]
            namespace = shard["metadata"]["namespace"]
            assert any(
                all(labels.get(k) == v for k, v in peer["podSelector"]["matchLabels"].items())
                and peer.get("namespaceSelector", {}).get("matchLabels", {}).get("kubernetes.io/metadata.name", policy["metadata"]["namespace"]) == namespace
                for peer in ingress["from"]
            ), f"{name} does not admit {shard['metadata']['name']}: {ingress['from']}"
