"""PF-47: Keycloak trusts the cluster's ServiceAccount tokens, so a workload needs no secret (T-1513).

The Portal's reconciler writes each workload-bound account's client with `federated-jwt`
authentication naming the realm's identity provider `kubernetes`; this is the other half:
the realm declares that provider with the cluster's issuer, Keycloak holds the one token it
reads the issuer's keys with (projected, an hour's life, the default token still unmounted),
and its NetworkPolicy reaches the API server on 6443; its only 443 to the internet is the
public-address rule for Client ID Metadata Documents (T-5954).
"""

import base64
import json

import pytest


@pytest.fixture(scope="module")
def realm(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return json.loads(base64.b64decode(doc["data"]["local.json"]))
    pytest.fail("no keycloak-config-cli realm Secret in the local render")


def keycloak_pod(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "StatefulSet" and doc["metadata"]["name"] == "keycloak-app-keycloakx":
            return doc["spec"]["template"]["spec"]
    pytest.fail("no Keycloak StatefulSet in the local render")


def test_the_realm_trusts_the_clusters_serviceaccount_issuer(realm):
    """PF-47: the alias the reconciler's clients name, of Keycloak's Kubernetes type."""
    providers = {p["alias"]: p for p in realm.get("identityProviders", [])}
    kubernetes = providers.get("kubernetes")
    assert kubernetes, sorted(providers)
    assert kubernetes["providerId"] == "kubernetes" and kubernetes["enabled"] is True
    assert kubernetes["config"]["issuer"] == "https://kubernetes.default.svc.cluster.local"


def test_keycloak_holds_a_short_projected_token_and_not_the_default_one(rendered):
    """PF-47, BSI TR-03187 AUT-3: the token the provider reads the issuer's keys with is a
    projected one of an hour; the automounted, non-expiring default stays off."""
    pod = keycloak_pod(rendered)
    account = next(
        d for d in rendered("local")
        if d.get("kind") == "ServiceAccount" and d["metadata"]["name"] == pod["serviceAccountName"]
    )
    assert account.get("automountServiceAccountToken") is False
    assert pod.get("automountServiceAccountToken") in (None, False)
    volumes = {v["name"]: v for v in pod["volumes"]}
    sources = volumes["kube-api-token"]["projected"]["sources"]
    token = next(s["serviceAccountToken"] for s in sources if "serviceAccountToken" in s)
    assert token["path"] == "token" and token["expirationSeconds"] <= 3600
    assert "audience" not in token, "the API server's own audience, so it opens nothing else"
    mounts = {
        m["name"]: m
        for c in pod["containers"]
        if c["name"] == "keycloak"
        for m in c.get("volumeMounts", [])
    }
    assert mounts["kube-api-token"]["mountPath"] == "/var/run/secrets/kubernetes.io/serviceaccount"
    assert mounts["kube-api-token"]["readOnly"] is True


PRIVATE = {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10"}


def test_keycloak_reaches_the_api_server_and_https_only_on_public_addresses(rendered):
    """PF-47: 6443 to the control plane. T-5954: a CIMD login fetches the client's document
    over https, so 443 opens, but only to public addresses: no pod, node, metadata service or
    loopback is reachable on it."""
    policies = [
        d for d in rendered("local")
        if d.get("kind") == "NetworkPolicy"
        and d["spec"].get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/instance") == "keycloak-app"
        and "Egress" in d["spec"].get("policyTypes", [])
    ]
    assert policies, "no egress policy selects the Keycloak pods"
    open_to_world = sorted(
        (port["port"], tuple(sorted(peer["ipBlock"].get("except", []))))
        for policy in policies
        for rule in policy["spec"].get("egress", [])
        for peer in rule.get("to", [])
        if peer.get("ipBlock", {}).get("cidr") == "0.0.0.0/0"
        for port in rule.get("ports", [])
    )
    # 6443 is the `kubeApi` peer: every address where global.kubeApiCidrs names none.
    assert open_to_world == [(443, tuple(sorted(PRIVATE | {"0.0.0.0/8", "198.18.0.0/15", "224.0.0.0/4"}))), (6443, ())], open_to_world
