"""The credential proxy of the builder runs on dev, judged as the deployment renders it (AG-49,
AG-52, AP-56).

What is coupled across files: the token the Portal and the proxy share, the port the proxy
calls the Portal on and the NetworkPolicy that opens it, the client id the proxy mints tokens
with and the ServiceAccount manifest the gateway resolves it to, and the profile name every run
loads and the file the forge seed commits."""

import base64
import json
import shutil

import pytest
import yaml
from conftest import seed_configmap

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def deployment(docs, name):
    return next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == name)


def container(docs, name):
    return deployment(docs, name)["spec"]["template"]["spec"]["containers"][0]


def env_of(c):
    return {e["name"]: e["value"] for e in c.get("env", []) if "value" in e}


def secret_env_of(c):
    return {
        e["name"]: (e["valueFrom"]["secretKeyRef"]["name"], e["valueFrom"]["secretKeyRef"]["key"])
        for e in c.get("env", [])
        if "valueFrom" in e and "secretKeyRef" in e["valueFrom"]
    }


@requires_helmfile
def test_the_proxy_runs_the_proxy_binary_of_the_platform_image_with_its_credentials_mounted(dev):
    proxy = container(dev, "agent-proxy")
    assert proxy["command"] == ["/usr/local/bin/jc-agent-proxy"]
    assert proxy["image"].startswith("ghcr.io/marek-mraz-jc/joinedcontext-platform:main@sha256:")
    env = env_of(proxy)
    assert env["JC_PORTAL_BASE"].endswith(":9090")
    assert env["JC_MODEL_PROVIDER"] == "openai-compatible"
    assert env["JC_OIDC_CLIENT_ID"] == "helsinki-agent-proxy"
    secrets = secret_env_of(proxy)
    # T-2271: no shared bearer any more. The proxy proves its own client with its pod's projected
    # token (PF-47, T-2868) and mints a token per audience from the in-cluster token endpoint,
    # which a pod can actually dial (T-2272).
    assert "JC_PROXY_TOKEN" not in secrets
    assert "JC_OIDC_CLIENT_SECRET" not in secrets
    assert env["JC_OIDC_CLIENT_ASSERTION_FILE"] == "/var/run/secrets/jc/keycloak/token"
    assert env["JC_OIDC_TOKEN_URL"].startswith("http://keycloak-app-keycloakx-http.")
    assert env["JC_OIDC_TOKEN_URL"].endswith("/protocol/openid-connect/token")
    assert secrets["JC_MODEL_KEY"] == ("agent-runner-model-key", "key")
    # No literal credential anywhere in the plain environment.
    assert not any(k.endswith(("KEY", "TOKEN", "SECRET")) for k in env)


@requires_helmfile
def test_the_proxy_probes_the_key_and_caps_the_days_tokens(dev):
    """T-3065, AG-96, AG-97: the key is probed every 15 minutes and every consumer and person has
    a daily token cap, as numbers the proxy parses; 0 is no cap (owner 2026-11-02, per person)."""
    env = env_of(container(dev, "agent-proxy"))
    assert env["JC_MODEL_PROBE_SECS"] == "900"
    caps = {name: env[f"JC_DAILY_TOKENS_{name}"] for name in ("ASSISTANT", "APP_BUILDER", "OTHER", "PER_PERSON")}
    assert caps == {"ASSISTANT": "2000000", "APP_BUILDER": "4000000", "OTHER": "1000000", "PER_PERSON": "0"}


@requires_helmfile
def test_the_portal_is_told_where_the_proxy_is_and_opens_its_internal_listener_to_it_alone(dev):
    portal = container(dev, "portal")
    env = env_of(portal)
    assert env["JC_AGENT_PROXY_BASE"] == "http://agent-proxy.dev.svc.cluster.local:8080"
    assert env["JC_AGENTS_NAMESPACE"] == "dev"
    # T-2271: the Portal is told whose token to expect on those routes instead of holding a copy
    # of the proxy's bearer.
    assert "JC_AGENT_PROXY_TOKEN" not in secret_env_of(portal)
    assert env_of(portal)["JC_PORTAL_AGENT_PROXY_CLIENT_ID"] == "helsinki-agent-proxy"
    service = next(d for d in dev if d.get("kind") == "Service" and d["metadata"]["name"] == "portal")
    assert {p["port"] for p in service["spec"]["ports"]} == {8080, 9090}
    policy = next(
        d for d in dev
        if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "portal-internal-from-agent-proxy"
    )
    (rule,) = policy["spec"]["ingress"]
    assert [p["port"] for p in rule["ports"]] == [9090]
    assert rule["from"][0]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "agent-runner-proxy"}


@requires_helmfile
def test_a_data_credential_is_honoured_from_the_functions_workload_with_its_namespace(dev):
    """F-17 (T-6662): the proxy compares the whole `<account>.<namespace>` of a caller's Linkerd
    identity, so the functions workload is named with the namespace it runs in."""
    env = env_of(container(dev, "agent-proxy"))
    functions = next(
        d for d in dev
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc-functions"
    )
    account = functions["spec"]["template"]["spec"].get("serviceAccountName") or "default"
    assert env["JC_PROXY_FUNCTIONS_IDENTITY"] == f"{account}.{functions['metadata']['namespace']}"


@requires_helmfile
def test_a_runs_ticket_is_honoured_from_the_portal_and_the_runs_own_meshed_workspace_alone(dev):
    """F-PLT1-008, T-5809: the proxy requires the caller's Linkerd identity and names the Portal's
    ServiceAccount with its namespace; the workspaces it serves reach it, and the Portal reaches
    them, on the service port and on the inbound proxy's 4143."""
    env = env_of(container(dev, "agent-proxy"))
    assert env["JC_REQUIRE_MESH_IDENTITY"] == "true"
    account = next(d for d in dev if d.get("kind") == "ServiceAccount" and d["metadata"]["name"] == "portal")
    assert env["JC_PROXY_PORTAL_IDENTITY"] == f"portal.{account['metadata']['namespace']}"
    assert container(dev, "portal") in [
        c for d in dev if d.get("kind") == "Deployment"
        and d["spec"]["template"]["spec"].get("serviceAccountName") == "portal"
        for c in d["spec"]["template"]["spec"]["containers"]
    ], "the Portal runs as the account the proxy names"

    workspace = {"matchExpressions": [{"key": "joinedcontext.com/run-id", "operator": "Exists"}]}

    def ports(name, direction):
        policy = next(d for d in dev if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == name)
        return {
            p["port"]
            for rule in policy["spec"][direction]
            for peer in rule.get("from" if direction == "ingress" else "to", [])
            if peer.get("podSelector") == workspace and peer.get("namespaceSelector")
            for p in rule["ports"]
        }

    assert 8080 in ports("agent-proxy", "ingress")
    assert 4143 in ports("agent-proxy-allow-linkerd", "ingress")
    assert ports("portal-egress-to-agent-workspaces", "egress") == {8000}
    assert ports("portal-linkerd-to-agent-workspaces", "egress") == {4143}


@requires_helmfile
def test_the_proxy_client_carries_every_endpoint_of_the_project_as_audience_and_a_manifest_names_it(dev):
    secret = next(
        d for d in dev
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "keycloak-config-keycloak-config-cli-config-realms"
    )
    realm = json.loads(base64.b64decode(secret["data"]["dev.json"]))
    client = next(c for c in realm["clients"] if c["clientId"] == "helsinki-agent-proxy")
    assert client["serviceAccountsEnabled"] and not client["publicClient"]
    assert not client["standardFlowEnabled"]
    audiences = {
        m["config"]["included.custom.audience"]
        for m in client["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    # The endpoint table the gateway serves is the forge's repository, seeded by the bootstrap
    # (T-0278); a key's `__` is a `/`.
    forge = seed_configmap(dev)
    documents = [
        yaml.safe_load(text)
        for key, text in forge["data"].items()
        # A bento.yaml beside a pipeline is the author's mapping, not a manifest (PL-03).
        if key.endswith(".yaml") and not key.endswith(".linkml.yaml") and not key.endswith("bento.yaml")
    ]
    # platform-settings.yaml is a plain document of the repository root, not a manifest: it
    # carries no kind (Architecture/06 §1, T-0901).
    manifests = [d for d in documents if isinstance(d, dict) and "kind" in d]
    slugs = {m["spec"]["slug"] for m in manifests if m["kind"] == "Endpoint" and m["metadata"]["namespace"] == "helsinki"}
    # Every seeded slug, and the gateway's own name for the endpoints approved after the realm
    # was written (Architecture/12 §5, T-0666); nothing else.
    # `portal-internal` is beside them since T-2271: the same client answers the Portal's internal
    # listener on the run callbacks, and one audience mapper per door.
    assert audiences == slugs | {"context-gateway", "portal-internal"}
    account = next(m for m in manifests if m["kind"] == "ServiceAccount" and m["metadata"]["name"] == "agent-proxy")
    assert account["spec"]["roles"] == [{"role": "public", "scope": {"contextSpace": "helsinki"}}]


@requires_helmfile
def test_the_forge_seed_commits_the_builder_profile(dev):
    seed = seed_configmap(dev)
    profile = yaml.safe_load(seed["data"]["agentprofiles__app-builder.yaml"])
    assert profile["kind"] == "AgentProfile"
    assert profile["spec"]["role"] == "builder"
    assert profile["spec"]["model"]["provider"] == "openai-compatible"
    # SDK-26, AG-72: the first run calls Gemini 3.8 Flash at medium reasoning.
    assert profile["spec"]["model"]["name"] == "google/gemini-3.8-flash"
    assert profile["spec"]["model"]["reasoningEffort"] == "medium"
    assert profile["spec"]["model"]["routing"]["order"][0] == "google-ai-studio/flex"
    # AG-70, AG-77: the assistant's share, KPI, space completion and change steps are granted,
    # never approval or removal.
    access = profile["spec"]["access"]
    # Everything that reads or proposes (T-1475); the person's own grants narrow each call.
    named = set(access["operations"])
    assert {"jc_resource_propose", "jc_change_list", "jc_pipeline_metrics", "jc_datasource_check",
            "jc_workspace_open", "jc_workspace_preview_start"} <= named
    # T-5957: the integrate-pipeline path (portal src/agents/paths.rs) samples, tries and validates.
    assert {"jc_pipeline_sample_source", "jc_pipeline_try_mapping", "jc_pipeline_validate"} <= named
    # T-6489: why a pipeline wrote nothing is read from its runs (portal paths.rs).
    assert {"jc_pipeline_logs", "jc_pipeline_inspect"} <= named
    # T-6533: a SyncSource pauses through its switch, and a member's bindings are found by name.
    assert {"jc_syncsource_pause", "jc_member_list"} <= named
    assert len(access["operations"]) == len(named), "an operation is listed once"
    # Never an agent's: approving, deleting, keys, whole projects, bringing a copy back (AG-11, AG-82).
    never = {"jc_change_approve", "jc_change_reject", "jc_resource_delete", "jc_project_delete",
             "jc_project_create", "jc_project_import", "jc_project_export", "jc_flow_start",
             "jc_draft_drop", "jc_workspace_discard", "jc_workspace_propose"}
    assert not named & never, named & never
    assert not any("_key_" in name for name in named)
    assert all(set(grant["verbs"]) <= {"read", "propose"} for grant in access["kinds"])
    assert {"Pipeline", "DataSource", "ContextSpace", "Role", "RoleBinding"} <= {grant["kind"] for grant in access["kinds"]}


@requires_helmfile
def test_the_forge_seed_commits_the_chat_profile_no_wider_than_the_builder(dev):
    seed = seed_configmap(dev)
    chat = yaml.safe_load(seed["data"]["agentprofiles__chat.yaml"])
    builder = yaml.safe_load(seed["data"]["agentprofiles__app-builder.yaml"])
    assert chat["kind"] == "AgentProfile" and chat["metadata"]["name"] == "chat"
    # T-2770, AG-72: conversations on a pool of their own, a steward with no tools and no
    # internet, and the reasoning that measured no thinking tokens ("none" is refused upstream).
    assert chat["spec"]["role"] == "steward"
    assert chat["spec"]["tools"] == []
    assert not chat["spec"].get("egress", {}).get("allowedHosts")
    assert chat["spec"]["model"]["reasoningEffort"] == "low"
    assert chat["spec"]["model"]["provider"] == "openai-compatible"
    assert chat["spec"]["limits"]["concurrentRunsPerOrganization"] >= builder["spec"]["limits"]["concurrentRunsPerOrganization"]
    # Security of T-2770: the chat's access is the builder's or narrower, never wider.
    # T-5957: the chat's integrate-pipeline path samples, tries and validates before it proposes.
    assert {"jc_pipeline_sample_source", "jc_pipeline_try_mapping", "jc_pipeline_validate"} <= set(chat["spec"]["access"]["operations"])
    assert set(chat["spec"]["access"]["operations"]) <= set(builder["spec"]["access"]["operations"])
    widest = {grant["kind"]: set(grant["verbs"]) for grant in builder["spec"]["access"]["kinds"]}
    for grant in chat["spec"]["access"]["kinds"]:
        assert set(grant["verbs"]) <= widest.get(grant["kind"], set()), grant
