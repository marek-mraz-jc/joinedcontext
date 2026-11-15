"""A read across brokers carries a token the gateway exchanged for the member brokers' audiences
(T-3999, ADR-N-047 §2.4, PF-113, PF-114).

One optional client scope per broker, `main` included, holds that broker's audience. The
context-gateway client alone exchanges and alone may request those scopes, so its own
client_credentials token names no broker and no other client's token ever does.
"""

import base64
import json

import pytest

from conftest import load_yaml_all
from test_context_brokers import BROKERS, OTHER, render

NAMES = ["main", *OTHER]


@pytest.fixture(scope="module")
def realm(tmp_path_factory):
    result = render(tmp_path_factory, BROKERS)
    assert result.returncode == 0, result.stderr
    docs = [d for d in load_yaml_all(result.out.read_text()) if isinstance(d, dict)]
    secret = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"].endswith("config-cli-config-realms"))
    return json.loads(base64.b64decode(secret["data"]["dev.json"]))


def audiences(mappers):
    return {
        m["config"].get("included.custom.audience") or m["config"].get("included.client.audience")
        for m in mappers or []
        if m["protocolMapper"] == "oidc-audience-mapper"
    }


def test_each_broker_main_included_has_a_scope_holding_its_audience_alone(realm):
    scopes = {s["name"]: s for s in realm["clientScopes"]}
    for name in NAMES:
        scope = scopes[f"context-broker-{name}"]
        assert audiences(scope["protocolMappers"]) == {f"context-broker-{name}"}
        assert scope["attributes"]["include.in.token.scope"] == "false"


def test_only_the_gateway_may_request_a_broker_scope_and_only_as_optional(realm):
    wanted = {f"context-broker-{name}" for name in NAMES}
    for client in realm["clients"]:
        default = set(client.get("defaultClientScopes", [])) & wanted
        assert not default, (client["clientId"], default)
        optional = set(client.get("optionalClientScopes", [])) & wanted
        if client["clientId"] == "context-gateway":
            assert optional == wanted
        else:
            assert not optional, (client["clientId"], optional)


def test_the_gateway_exchanges_and_its_own_token_names_no_broker(realm):
    gateway = next(c for c in realm["clients"] if c["clientId"] == "context-gateway")
    assert gateway["attributes"]["standard.token.exchange.enabled"] == "true"
    # No refresh token for a federated read: the exchanged token lives for one forward.
    assert "standard.token.exchange.enableRefreshRequestedTokenType" not in gateway["attributes"]
    assert not {a for a in audiences(gateway["protocolMappers"]) if a and a.startswith("context-broker-")}


def test_a_persons_token_names_the_gateway_so_it_may_be_exchanged(realm):
    """V2 exchanges a subject token only when its audience names the requesting client."""
    edge = next(c for c in realm["clients"] if c["clientId"] == "edge")
    assert "context-gateway" in audiences(edge["protocolMappers"])
