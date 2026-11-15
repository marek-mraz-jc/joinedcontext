"""Who may call the pipeline runner's streams API inside the mesh (T-2568, PL-07, PF-46).

A meshed call arrives on the runner's Linkerd inbound port 4143, which
`pipeline-runner-allow-linkerd` opens to every pod, so the NetworkPolicy on 4195 never sees it.
The runner's environment holds the pipeline secrets, and a stream `PUT` runs with it. The Linkerd
`Server` on 4195 and its `AuthorizationPolicy` are what keep every identity but the Portal's out.

Conftest reads one document at a time in CI, so the pairing of the 4143 opening with the Server
is checked here, over the whole render.
"""

import pytest

from conftest import set_global

ENVIRONMENTS = ("local", "dev", "production")
RUNNER = "pipeline-runner-runner"


def of_kind(docs, kind):
    return [d for d in docs if d.get("kind") == kind and d.get("apiVersion", "").startswith("policy.linkerd.io/")]


def runner_server(docs):
    servers = [
        s
        for s in of_kind(docs, "Server")
        if s["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/name") == RUNNER
    ]
    assert len(servers) == 1, [s["metadata"]["name"] for s in servers]
    return servers[0]


def portal_namespace(docs):
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    return portal["metadata"]["namespace"]


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_only_the_portal_s_identity_is_authorized_on_the_streams_api(rendered, environment):
    docs = rendered(environment)
    server = runner_server(docs)
    assert server["spec"]["port"] == 4195
    assert server["spec"]["accessPolicy"] == "deny", "what the policy does not name is refused"

    policies = [
        p for p in of_kind(docs, "AuthorizationPolicy") if p["spec"]["targetRef"]["name"] == server["metadata"]["name"]
    ]
    assert len(policies) == 1
    assert policies[0]["spec"]["targetRef"]["kind"] == "Server"
    (required,) = policies[0]["spec"]["requiredAuthenticationRefs"]
    assert required["kind"] == "MeshTLSAuthentication"

    authentication = next(m for m in of_kind(docs, "MeshTLSAuthentication") if m["metadata"]["name"] == required["name"])
    assert authentication["spec"]["identityRefs"] == [
        {"kind": "ServiceAccount", "name": "portal", "namespace": portal_namespace(docs)}
    ]
    # Every policy object lives beside the runner it guards.
    runner = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    for obj in (server, policies[0], authentication):
        assert obj["metadata"]["namespace"] == runner["metadata"]["namespace"]


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_named_identity_is_the_portal_s_own_service_account(rendered, environment):
    docs = rendered(environment)
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    assert portal["spec"]["template"]["spec"]["serviceAccountName"] == "portal"
    assert any(
        d.get("kind") == "ServiceAccount" and d["metadata"]["name"] == "portal" for d in docs
    ), "the identity the runner admits exists"


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_opening_the_proxy_port_to_everyone_comes_with_the_server(rendered, environment):
    """The NetworkPolicy that admits any pod to 4143 is safe only beside the Server on the app
    port; the day one appears without the other, the streams API is open to the mesh again."""
    docs = rendered(environment)
    opens_the_proxy = [
        p
        for p in docs
        if p.get("kind") == "NetworkPolicy"
        and p["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/name") == RUNNER
        and any(
            "from" not in rule and any(port.get("port") == 4143 for port in rule.get("ports", []))
            for rule in p["spec"].get("ingress", [])
        )
    ]
    assert opens_the_proxy, "the runner is meshed in this environment"
    runner_server(docs)


def test_an_unmeshed_runner_gets_no_mesh_policy(rendered_variant):
    docs = rendered_variant("dev", lambda tree: set_global(tree, "serviceMesh.enable", False))
    assert of_kind(docs, "MeshTLSAuthentication") == []
    assert of_kind(docs, "AuthorizationPolicy") == []
    assert not any(
        s["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/name") == RUNNER
        for s in of_kind(docs, "Server")
    )


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_no_networkpolicy_admits_a_peer_to_the_streams_api(rendered, environment):
    """T-4044: off the mesh, 4195 is reached by no pod at all (PL-07): every runner ingress rule
    either opens the Linkerd proxy port, which the Server above narrows to the Portal, or none."""
    docs = rendered(environment)
    for policy in docs:
        if policy.get("kind") != "NetworkPolicy":
            continue
        if policy["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/name") != RUNNER:
            continue
        for rule in policy["spec"].get("ingress", []):
            opened = {port.get("port") for port in rule.get("ports", [])}
            assert opened and 4195 not in opened, (policy["metadata"]["name"], rule)
