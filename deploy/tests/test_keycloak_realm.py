"""T-5568, TR-03187 AR-6: the realm ends a removed person's access within seconds.

Keycloak posts the end of every session of the edge and the Portal clients to the Portal, which
refuses the person's tokens there and, through its revocation list, on the gateway. A refresh
token works once. An offline token is bounded to 60 days however often it is used, and only the
MCP connector clients may ask for one.

`offline_access` stays a default realm role: Keycloak issues an offline token only to a user
holding that role through a client whose scopes include it, so the role cannot be granted per
client. The client scope is what is narrowed.
"""

import base64
import json

import pytest

OFFLINE_CLIENTS = {"mcp-mobile", "mcp-hub"}


@pytest.fixture(scope="module")
def realm(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return json.loads(base64.b64decode(doc["data"]["local.json"]))
    pytest.fail("no keycloak-config-cli realm Secret in the local render")


def portal_namespace(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "Service" and doc["metadata"]["name"] == "portal":
            return doc["metadata"]["namespace"]
    pytest.fail("no portal Service in the local render")


def test_clients_carry_backchannel_logout_and_offline_is_bounded(realm, rendered):
    assert realm["revokeRefreshToken"] is True
    assert realm["refreshTokenMaxReuse"] == 0
    assert realm["offlineSessionMaxLifespanEnabled"] is True
    assert realm["offlineSessionMaxLifespan"] == 60 * 24 * 3600

    clients = {c["clientId"]: c for c in realm["clients"]}
    url = f"http://portal.{portal_namespace(rendered)}.svc.cluster.local:8080/api/v1/auth/backchannel-logout"
    for client_id in ("edge", "portal-api"):
        attributes = clients[client_id]["attributes"]
        assert attributes.get("backchannel.logout.url") == url, client_id
        assert attributes.get("backchannel.logout.session.required") == "true", client_id
        # The merge of the client's own attributes keeps what the defaults carry.
        assert attributes["pkce.code.challenge.method"] == "S256", client_id

    offline = {
        client_id
        for client_id, client in clients.items()
        if "offline_access" in client.get("optionalClientScopes", []) + client.get("defaultClientScopes", [])
    }
    assert offline <= OFFLINE_CLIENTS, offline - OFFLINE_CLIENTS


def test_no_other_client_posts_its_logouts_to_the_portal(realm):
    posting = {c["clientId"] for c in realm["clients"] if "backchannel.logout.url" in c.get("attributes", {})}
    assert posting == {"edge", "portal-api"}
