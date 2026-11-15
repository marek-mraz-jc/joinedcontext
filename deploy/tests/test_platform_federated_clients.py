"""PF-47, T-2868: a platform workload whose Keycloak client is `federated` holds no client secret.

A client in `components/*/keycloak-clients.yaml` with a `federated` block is rendered into the
realm as a `federated-jwt` client trusting the projected token of one ServiceAccount, no secret
is generated for it, Keycloak is handed none, and the pod proves itself with a short projected
token whose audience is the realm. Read from the files, so a component that moves later is
checked by the same tests.
"""

import base64
import json
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent


def federated_clients() -> dict[str, dict]:
    out = {}
    for path in sorted((ROOT / "components").glob("*/keycloak-clients.yaml")):
        for client_id, client in (yaml.safe_load(path.read_text()) or {}).items():
            if client.get("federated"):
                out[client_id] = client["federated"]
    return out


@pytest.fixture(scope="module")
def docs(rendered):
    # `dev`: the agent runner and the assistant are rendered there and not in `local`.
    return rendered("dev")


@pytest.fixture(scope="module")
def realm(docs):
    for doc in docs:
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            data = json.loads(base64.b64decode(doc["data"]["dev.json"]))
            return {client["clientId"]: client for client in data["clients"]}
    pytest.fail("no keycloak-config-cli realm Secret in the dev render")


def workload(docs, kind: str, name: str) -> dict:
    for doc in docs:
        if doc.get("kind") == kind and doc["metadata"]["name"] == name:
            return doc
    pytest.fail(f"no {kind} {name} in the dev render")


def test_the_platforms_own_workloads_are_federated():
    """T-2868: the gateway, the agent proxy and the assistant hold no client secret."""
    clients = federated_clients()
    assert clients.get("context-gateway") == {"release": "context-gateway.gateway", "serviceAccount": "context-gateway"}
    assert clients.get("helsinki-agent-proxy") == {"release": "agent-runner.proxy", "serviceAccount": "agent-proxy"}
    assert clients.get("jc-assistant") == {"release": "assistant.worker", "serviceAccount": "jc-assistant"}
    assert clients.get("jc-build-lane") == {"release": "gitea.bootstrap", "serviceAccount": "gitea-bootstrap-lane-token"}


@pytest.mark.parametrize("client_id", sorted(federated_clients()))
def test_the_pod_of_a_federated_client_mounts_a_short_realm_bound_token_and_no_secret(client_id, docs):
    """The pod running as the client's ServiceAccount presents its projected token: audience the
    realm, at most an hour, read-only, and the `*_ASSERTION_FILE` it is told to read is that file.
    `portal-reconciler` runs no pod; the Portal mints its token (test below)."""
    federated = federated_clients()[client_id]
    pods = [
        d for d in docs
        if d.get("kind") in ("Deployment", "StatefulSet")
        and d["spec"]["template"]["spec"].get("serviceAccountName") == federated["serviceAccount"]
    ]
    if federated["serviceAccount"] == "gitea-bootstrap-lane-token":
        return  # a CronJob with a script of its own: tests/test_lane_token.py
    if federated["serviceAccount"] == "portal-reconciler":
        assert pods == []
        return
    assert len(pods) == 1, f"one workload runs as {federated['serviceAccount']}, not {len(pods)}"
    pod = pods[0]["spec"]["template"]["spec"]
    issuers = {e["value"] for c in pod["containers"] for e in c.get("env", []) if e["name"] == "JC_OIDC_ISSUER"}
    files = {
        e["value"]: c for c in pod["containers"] for e in c.get("env", [])
        if e["name"].endswith("CLIENT_ASSERTION_FILE")
    }
    assert len(files) == 1, files
    ((path, container),) = files.items()
    path = Path(path)
    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == str(path.parent))
    assert mount.get("readOnly") is True
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    token = next(s["serviceAccountToken"] for s in volume["projected"]["sources"] if "serviceAccountToken" in s)
    assert token["path"] == path.name
    assert token["audience"].startswith("https://idm.") and "/realms/" in token["audience"]
    if issuers:
        assert {token["audience"]} == issuers, "bound to the realm and nothing else"
    assert 600 <= token["expirationSeconds"] <= 3600
    named = {
        e["valueFrom"]["secretKeyRef"]["name"]
        for c in pod["containers"] for e in c.get("env", []) if "secretKeyRef" in e.get("valueFrom", {})
    } | {v["secret"]["secretName"] for v in pod.get("volumes", []) if "secret" in v}
    assert f"keycloak-client-{client_id}" not in named, named


def test_the_portals_two_clients_are_federated_as_two_subjects():
    """T-2868: the login client trusts the pod's account, the reconciler's client one of its own,
    because Keycloak finds a federated client by the token's subject."""
    clients = federated_clients()
    assert clients.get("portal-api") == {"release": "portal.portal", "serviceAccount": "portal"}
    assert clients.get("portal-reconciler") == {"release": "portal.portal", "serviceAccount": "portal-reconciler"}


@pytest.mark.parametrize("client_id", sorted(federated_clients()))
def test_a_federated_client_trusts_one_serviceaccount_and_holds_no_secret(client_id, docs, realm):
    federated = federated_clients()[client_id]
    client = realm[client_id]
    assert client["clientAuthenticatorType"] == "federated-jwt"
    assert "secret" not in client, "a federated client must not carry a secret beside it"
    assert client["attributes"]["jwt.credential.issuer"] == "kubernetes"
    accounts = [
        doc for doc in docs
        if doc.get("kind") == "ServiceAccount" and doc["metadata"]["name"] == federated["serviceAccount"]
    ]
    assert len(accounts) == 1, f"one ServiceAccount {federated['serviceAccount']}, not {len(accounts)}"
    namespace = accounts[0]["metadata"]["namespace"]
    assert client["attributes"]["jwt.credential.sub"] == (
        f"system:serviceaccount:{namespace}:{federated['serviceAccount']}"
    )
    secrets = {doc["metadata"]["name"] for doc in docs if doc.get("kind") == "Secret"}
    assert f"keycloak-client-{client_id}" not in secrets
    keycloak = workload(docs, "StatefulSet", "keycloak-app-keycloakx")["spec"]["template"]["spec"]
    env = {e["name"] for c in keycloak["containers"] for e in c.get("env", [])}
    assert f"CLIENT_SECRET_{client_id.upper().replace('-', '_')}" not in env


def test_the_portal_proves_itself_with_a_short_projected_token_and_mounts_no_client_secret(docs):
    pod = workload(docs, "Deployment", "portal")["spec"]["template"]["spec"]
    container = next(
        c for c in pod["containers"] if any(e["name"] == "JC_OIDC_CLIENT_ID" for e in c.get("env", []))
    )
    env = {e["name"]: e for e in container.get("env", [])}
    assert "JC_OIDC_CLIENT_SECRET" not in env
    secret_refs = {
        e["valueFrom"]["secretKeyRef"]["name"]
        for e in container.get("env", [])
        if "secretKeyRef" in e.get("valueFrom", {})
    }
    assert "keycloak-client-portal-api" not in secret_refs, secret_refs
    assert not any(
        v.get("secret", {}).get("secretName") == "keycloak-client-portal-api" for v in pod.get("volumes", [])
    )

    path = Path(env["JC_OIDC_CLIENT_ASSERTION_FILE"]["value"])
    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == str(path.parent))
    assert mount.get("readOnly") is True
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    token = next(s["serviceAccountToken"] for s in volume["projected"]["sources"] if "serviceAccountToken" in s)
    assert token["path"] == path.name
    assert token["audience"] == env["JC_OIDC_ISSUER"]["value"], "bound to the realm and nothing else"
    assert 600 <= token["expirationSeconds"] <= 3600


def test_the_portal_mints_the_reconcilers_token_for_that_account_and_no_other(docs):
    """PF-47: the Portal may create a token of `portal-reconciler` and of nothing else; the
    account runs no pod and mounts no token of its own."""
    portal = workload(docs, "Deployment", "portal")
    namespace = portal["metadata"]["namespace"]
    pod = portal["spec"]["template"]["spec"]
    container = next(
        c for c in pod["containers"] if any(e["name"] == "JC_OIDC_CLIENT_ID" for e in c.get("env", []))
    )
    env = {e["name"]: e for e in container.get("env", [])}
    assert env["JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SERVICE_ACCOUNT"]["value"] == "portal-reconciler"
    assert "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET" not in env

    account = next(
        d for d in docs
        if d.get("kind") == "ServiceAccount" and d["metadata"]["name"] == "portal-reconciler"
    )
    assert account["metadata"]["namespace"] == namespace
    assert account.get("automountServiceAccountToken") is False
    role = workload(docs, "Role", "portal-reconciler-token")
    assert role["metadata"]["namespace"] == namespace
    assert role["rules"] == [
        {
            "apiGroups": [""],
            "resources": ["serviceaccounts/token"],
            "verbs": ["create"],
            "resourceNames": ["portal-reconciler"],
        }
    ]
    binding = workload(docs, "RoleBinding", "portal-reconciler-token")
    assert binding["roleRef"]["name"] == "portal-reconciler-token"
    assert binding["subjects"] == [
        {"kind": "ServiceAccount", "name": pod["serviceAccountName"], "namespace": namespace}
    ]
    assert not any(
        d.get("kind") in ("Deployment", "StatefulSet")
        and d["spec"]["template"]["spec"].get("serviceAccountName") == "portal-reconciler"
        for d in docs
    )
