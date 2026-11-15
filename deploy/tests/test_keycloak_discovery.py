"""T-5611: the realm's discovery documents leave the edge without `registration_endpoint`.

An MCP client that finds the member registers itself before it signs anybody in; the realm
refuses an anonymous registration (403), so the person saw a failed connection instead of the
field for the fixed client id. The `keycloak-discovery` route rewrites the four discovery paths
and keeps the plugin config's security headers. The last test runs the pinned APISIX image with
the committed route, because only an answer proves the filter and the path match.
"""

import json
from pathlib import Path

import yaml
from test_apisix_429_contract import answer, pinned_apisix

ROOT = Path(__file__).resolve().parent.parent
ROUTES = ROOT / "components/keycloak/apisix-routes.yaml"
PLUGINS = ROOT / "components/keycloak/apisix-plugins.yaml"
ISSUER = "https://idm.example.org/realms/dev"
# The shape Keycloak 26.8 answers, both copies of the member included (top level and mTLS aliases).
DOCUMENT = {
    "issuer": ISSUER,
    "authorization_endpoint": f"{ISSUER}/protocol/openid-connect/auth",
    "token_endpoint": f"{ISSUER}/protocol/openid-connect/token",
    "registration_endpoint": f"{ISSUER}/clients-registrations/openid-connect",
    "code_challenge_methods_supported": ["plain", "S256"],
    "mtls_endpoint_aliases": {
        "token_endpoint": f"{ISSUER}/protocol/openid-connect/token",
        "registration_endpoint": f"{ISSUER}/clients-registrations/openid-connect",
    },
}
DISCOVERY = [
    "/.well-known/oauth-authorization-server/realms/dev",
    "/.well-known/openid-configuration/realms/dev",
    "/realms/dev/.well-known/openid-configuration",
    "/realms/dev/.well-known/oauth-authorization-server",
]
ELSEWHERE = ["/realms/dev/account/.well-known/openid-configuration", "/realms/dev/protocol/openid-connect/certs"]


def load(path: Path) -> dict:
    # The route files are Helm templates; the `{{ … }}` upstreams are plain strings to YAML.
    return yaml.safe_load(path.read_text())


def test_the_discovery_route_keeps_the_keycloak_chain_and_its_headers():
    route, page = load(ROUTES)["keycloak-discovery"], load(ROUTES)["keycloak"]
    assert (route["subDomain"], route["upstream"], route["pluginConfig"]) == (page["subDomain"], page["upstream"], "keycloak")
    assert route["methods"] == ["GET", "HEAD"], "a registration POST is never rewritten"
    assert set(route["plugins"]) == {"response-rewrite"}, "every other plugin is the config's"
    mine = route["plugins"]["response-rewrite"]
    theirs = load(PLUGINS)["keycloak"]["plugins"]["response-rewrite"]
    assert mine["headers"] == theirs["headers"], "the security headers, word for word"
    assert mine["filters"] == [{"regex": ',"registration_endpoint":"[^"]*"', "scope": "global", "replace": ""}]


def test_the_pinned_image_drops_the_registration_endpoint_from_discovery_alone(tmp_path):
    route = load(ROUTES)["keycloak-discovery"]
    discovery = yaml.safe_dump({
        "uri": route["uri"], "priority": route["priority"], "methods": route["methods"], "vars": route["vars"],
        "plugins": {**route["plugins"], "proxy-rewrite": {"uri": "/keycloak"}},
        "upstream": {"nodes": {"127.0.0.1:9080": 1}, "type": "roundrobin"},
    }, width=1000)
    rules = "routes:\n  - " + discovery.replace("\n", "\n    ").rstrip() + f"""
  - uri: /*
    plugins:
      proxy-rewrite:
        uri: /keycloak
    upstream:
      nodes:
        "127.0.0.1:9080": 1
      type: roundrobin
  - uri: /keycloak
    priority: 10
    plugins:
      mocking:
        response_status: 200
        content_type: application/json
        response_example: {json.dumps(json.dumps(DOCUMENT, separators=(",", ":")))}
    upstream:
      nodes:
        "127.0.0.1:9": 1
      type: roundrobin
#END
"""
    with pinned_apisix(tmp_path, rules) as base:
        for path in DISCOVERY:
            status, headers, body = answer(base + path)
            assert status == 200, (path, body)
            served = json.loads(body)
            assert "registration_endpoint" not in served and "registration_endpoint" not in served["mtls_endpoint_aliases"], path
            kept = {k: v for k, v in DOCUMENT.items() if k not in ("registration_endpoint", "mtls_endpoint_aliases")}
            assert {k: served[k] for k in kept} == kept, path
            assert headers["X-Content-Type-Options"] == "nosniff", path
        for path in ELSEWHERE:
            status, _, body = answer(base + path)
            assert status == 200 and "registration_endpoint" in json.loads(body), f"{path} is not a discovery document"

