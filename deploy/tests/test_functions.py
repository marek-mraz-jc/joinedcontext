"""jc-functions on dev, judged as the deployment renders it (SDK-22, SDK-23, T-0686).

What is coupled across files: the audience and caller the runtime accepts and the mapper on the
Portal's client that puts that audience in its token, the Service the Portal is pointed at, and
the NetworkPolicies that let the Portal in and the runtime out to the gateway and Keycloak only."""

import base64
import json
import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@requires_helmfile
def test_the_runtime_is_the_functions_binary_with_no_credential(dev):
    (container,) = one(dev, "Deployment", "jc-functions")["spec"]["template"]["spec"]["containers"]
    assert container["command"] == ["/usr/local/bin/jc-functions"]
    assert container["image"].startswith("ghcr.io/marek-mraz-jc/joinedcontext-platform:main@sha256:")
    env = env_of(container)
    assert env["JC_FUNCTIONS_CALLER"] == "portal-api"
    assert env["JC_FUNCTIONS_AUDIENCE"] == "jc-functions"
    assert env["JC_GATEWAY_URL"] == "http://context-gateway.dev.svc.cluster.local:8080"
    assert env["JC_OIDC_JWKS_URL"].startswith("http://keycloak-app-keycloakx-http.")
    assert all("valueFrom" not in e for e in container["env"]), "the runtime holds no secret"
    assert container["resources"]["limits"]["memory"] == "1Gi"


@requires_helmfile
def test_the_portal_calls_the_service_with_a_token_the_runtime_accepts(dev):
    portal = one(dev, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]
    assert env_of(portal)["JC_FUNCTIONS_URL"] == "http://jc-functions.dev.svc.cluster.local:8080"
    service = one(dev, "Service", "jc-functions")
    assert [p["port"] for p in service["spec"]["ports"]] == [8080]
    secret = one(dev, "Secret", "keycloak-config-keycloak-config-cli-config-realms")
    realm = json.loads(base64.b64decode(secret["data"]["dev.json"]))
    client = next(c for c in realm["clients"] if c["clientId"] == "portal-api")
    assert client["serviceAccountsEnabled"]
    audiences = {
        m["config"]["included.custom.audience"]
        for m in client["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    # jc-functions for the Portal's own calls; helsinki-agent-proxy so a person's token is the
    # subject token the agent proxy exchanges for a run of theirs (T-2866, ADR-N-038). No other.
    # and jc-assistant, whose administration paths the Portal asks with its own token (T-3057),
    # and context-gateway-simulate, the one audience the gateway's access/simulate takes (T-3311).
    assert audiences == {"jc-functions", "helsinki-agent-proxy", "jc-assistant", "context-gateway-simulate"}


@requires_helmfile
def test_only_the_portal_gets_in_and_the_runtime_reaches_the_gateway_keycloak_and_dns_only(dev):
    policy = one(dev, "NetworkPolicy", "functions-runtime")
    assert policy["spec"]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "functions-runtime"}
    (inbound,) = policy["spec"]["ingress"]
    assert [f["podSelector"]["matchLabels"] for f in inbound["from"]] == [{"app.kubernetes.io/name": "portal-portal"}]
    assert [p["port"] for p in inbound["ports"]] == [8080]
    targets = [
        (rule["to"][0]["podSelector"]["matchLabels"], [p["port"] for p in rule["ports"]])
        for rule in policy["spec"]["egress"]
    ]
    assert targets == [
        ({"app.kubernetes.io/name": "context-gateway-gateway"}, [8080]),
        ({"app.kubernetes.io/instance": "keycloak-app"}, [8080]),
        ({"k8s-app": "kube-dns"}, [53, 53]),
    ]
    workload = one(dev, "Deployment", "jc-functions")["spec"]["template"]["metadata"]["labels"]
    assert workload["app.kubernetes.io/name"] == "functions-runtime"
    for name in ("portal-egress-to-functions", "gateway-from-functions", "keycloak-from-functions"):
        one(dev, "NetworkPolicy", name)
