"""The public data host `api.{domain}` (ADR-N-052 §2.1, EP-128, T-4630).

The four surfaces that hand data to the outside, `/api/endpoint/*`, `/api/mcp*`, `/cs/*` and
`/catalog*`, and the gateway's anonymous OAuth server answer on `api.{domain}`, each on the apex
route's own chain. An anonymous request for one of them on the apex or the Portal host is a 308
to the same path and query there; a request with a bearer, or with the Portal's edge session, is
still served where it is. The last test runs the pinned APISIX image with the committed redirect,
because what a client is answered is only proved by an answer; it is skipped without docker.
"""

import http.client
from pathlib import Path

import pytest
import yaml

from test_apisix_429_contract import pinned_apisix
from test_apisix_routes import LOCAL_DOMAIN, PORTAL_HOST, hosts_of, requires_helmfile

API_HOST = f"api.{LOCAL_DOMAIN}"
COMPONENT = Path(__file__).resolve().parent.parent / "components/context-gateway"
# Each data-host route and the apex route whose chain it shares.
ON_THE_DATA_HOST = {
    "api-context-space": ("/cs/*", "context-space"),
    "api-context-space-read": ("/cs/*", "context-space-read"),
    "api-catalog-feed": ("/catalog*", "catalog-feed"),
    "api-context-endpoint": ("/api/endpoint/*", "context-endpoint"),
    "api-context-endpoint-ws": ("/api/endpoint/*", "context-endpoint-ws"),
    "api-mcp-hub": ("/api/mcp*", "context-mcp-hub"),
    "api-mcp-named": ("/api/mcp/*", "context-mcp-named"),
}
MOVED = {"endpoint": "/api/endpoint/*", "mcp": "/api/mcp*", "cs": "/cs/*", "catalog": "/catalog*"}
# Every prefix the Portal host moves; `/catalog*` would also match its own `/catalogue` page.
PORTAL_MOVED = {"endpoint", "mcp", "cs"}
NO_BEARER = ["http_authorization", "!", "~*", "^bearer\\s+\\S"]
NO_EDGE_SESSION = ["cookie___Host-jc_edge", "!", "~~", "."]


def parsed_routes(apisix_config):
    _, parsed = apisix_config
    routes = {r["id"]: r for r in parsed["routes"]}
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    upstreams = {u["id"]: u for u in parsed["upstreams"]}
    return routes, plugins, upstreams


@pytest.fixture(scope="module")
def apisix_config(rendered):
    docs = rendered("local")
    cm = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base")
    raw = cm["data"]["apisix.yaml"]
    return raw, yaml.safe_load(raw)


@requires_helmfile
def test_the_data_host_serves_the_four_surfaces_on_the_apex_chains(apisix_config):
    routes, _, upstreams = parsed_routes(apisix_config)
    for route_id, (uri, apex_id) in ON_THE_DATA_HOST.items():
        route, apex = routes[route_id], routes[apex_id]
        assert hosts_of(route) == [API_HOST] and hosts_of(apex) == [LOCAL_DOMAIN], route_id
        assert route["uri"] == apex["uri"] == uri, route_id
        assert route["priority"] == apex["priority"], route_id
        assert route.get("plugin_config_id") == apex.get("plugin_config_id"), route_id
        assert route.get("vars") == apex.get("vars") and route.get("methods") == apex.get("methods"), route_id
        # T-5900: the data host alone keeps the edge cache of anonymous reads (test_edge_api_cache.py).
        # T-6185: with buffering back on, or proxy-cache stores nothing.
        cache = {"proxy-buffering", "proxy-cache"} if route_id == "api-context-endpoint" else set()
        assert set(route.get("plugins", {})) == set(apex.get("plugins", {})) | cache, route_id
        assert {k: v for k, v in route.get("plugins", {}).items() if k not in cache} == apex.get("plugins", {}), route_id
        assert all(node.startswith("context-gateway.") for node in upstreams[route_id]["nodes"]), route_id
    assert routes["api-context-endpoint-ws"].get("enable_websocket") is True


@requires_helmfile
def test_the_oauth_server_answers_on_the_data_host_alone_and_counts_per_address(apisix_config):
    """ADR-N-052 §2.2 and §4: the gateway's `/oauth/*` and its `/.well-known/oauth-*` documents,
    on the endpoint chain; minting is limited per client address at the edge."""
    routes, _, upstreams = parsed_routes(apisix_config)
    oauth, metadata = routes["api-oauth"], routes["api-oauth-metadata"]
    assert (oauth["uri"], metadata["uri"]) == ("/oauth/*", "/.well-known/oauth-*")
    for route in (oauth, metadata):
        assert hosts_of(route) == [API_HOST]
        assert route["plugin_config_id"] == routes["context-endpoint"]["plugin_config_id"]
    limit = oauth["plugins"]["limit-count"]
    assert (limit["count"], limit["time_window"], limit["key"]) == (30, 60, "remote_addr")
    assert sorted(oauth["methods"]) == ["GET", "POST"] and sorted(metadata["methods"]) == ["GET", "HEAD"]
    gateway = {u_id for u_id, u in upstreams.items() if all(node.startswith("context-gateway.") for node in u["nodes"])}
    elsewhere = [r["id"] for r in routes.values() if r["upstream_id"] in gateway and API_HOST not in hosts_of(r)
                 and r["uri"].startswith(("/oauth", "/.well-known/oauth-authorization-server"))]
    assert not elsewhere


@requires_helmfile
def test_an_anonymous_old_address_is_a_308_to_the_same_path_on_the_data_host(apisix_config):
    routes, plugins, _ = parsed_routes(apisix_config)
    assert plugins["api-moved"]["redirect"] == {"uri": f"https://{API_HOST}$request_uri", "ret_code": 308}
    assert "openid-connect" not in plugins["api-moved"]
    # The Portal host never served the feed, and `/catalog*` there would take its `/catalogue` page.
    assert "api-moved-portal-catalog" not in routes
    for name, uri in MOVED.items():
        apex, portal = routes[f"api-moved-{name}"], routes.get(f"api-moved-portal-{name}")
        assert hosts_of(apex) == [LOCAL_DOMAIN] and apex["vars"] == [NO_BEARER], name
        assert (portal is not None) == (name in PORTAL_MOVED), name
        if portal:
            assert hosts_of(portal) == [PORTAL_HOST] and portal["vars"] == [NO_BEARER, NO_EDGE_SESSION], name
        for moved in filter(None, (apex, portal)):
            assert moved["uri"] == uri and moved["plugin_config_id"] == "api-moved", name
            # Above every route of its host that serves the same prefix, so anonymous never reaches them.
            same = [r for r in routes.values() if r["uri"] == uri and hosts_of(r) == hosts_of(moved) and r is not moved]
            assert same and all(moved["priority"] > r["priority"] for r in same), name


@requires_helmfile
def test_the_data_host_is_on_the_edge_certificate_and_the_ingress(rendered):
    docs = rendered("local")
    certificate = next(d for d in docs if d.get("kind") == "Certificate" and d["metadata"]["name"] == "apisix-edge")
    assert API_HOST in certificate["spec"]["dnsNames"]
    assert not [name for name in certificate["spec"]["dnsNames"] if name.startswith("*.api")]
    ingress_hosts = [rule["host"] for d in docs if d.get("kind") == "Ingress" for rule in d["spec"].get("rules", [])]
    assert API_HOST in ingress_hosts


@requires_helmfile
def test_the_gateway_names_the_data_host_and_reads_its_oauth_key_from_a_secret(rendered):
    docs = rendered("local")
    gateway = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "context-gateway")
    env = {e["name"]: e for c in gateway["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_GATEWAY_PUBLIC_URL"]["value"] == f"https://{API_HOST}"
    # OAuth is for Gemini only (owner 2026-10-18, ADR-N-052 §2.3): no other assistant's host.
    assert env["JC_GATEWAY_OAUTH_REDIRECT_HOSTS"]["value"] == "oauth-redirect.googleusercontent.com"
    key = env["JC_GATEWAY_OAUTH_KEY"]
    assert "value" not in key and key["valueFrom"]["secretKeyRef"] == {"name": "context-gateway-oauth-key", "key": "key"}
    secret = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"] == "context-gateway-oauth-key")
    assert set(secret.get("data") or secret.get("stringData")) == {"key"}
    generated = yaml.safe_load((COMPONENT / "secrets.yaml").read_text())["context-gateway"]["gateway"]["context-gateway-oauth-key"]
    # ADR-N-052 §2.2.6: at least 32 bytes, generated, never typed into Git.
    assert generated["key"]["generate"] is True and generated["key"]["length"] >= 32


def ask(port: int, path: str, headers: dict[str, str]) -> tuple[int, str | None]:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    connection.request("POST", path, body=b"{}", headers={"Host": LOCAL_DOMAIN, **headers})
    response = connection.getresponse()
    response.read()
    return response.status, response.getheader("Location")


def test_the_pinned_image_moves_an_anonymous_call_and_serves_a_bearer(tmp_path):
    """The committed chain's `redirect` with `$request_uri` keeps the path and the query and
    answers 308, so an MCP POST is replayed on the data host with its body; a bearer is not
    moved, it takes the apex route (here a closed upstream, a 502)."""
    plugins = yaml.safe_load((COMPONENT / "apisix-plugins.yaml").read_text())["api-moved"]["plugins"]
    redirect = dict(plugins["redirect"], uri=plugins["redirect"]["uri"].replace("${DOMAIN}", LOCAL_DOMAIN))
    rules = {
        "upstreams": [{"id": "closed", "type": "roundrobin", "nodes": {"127.0.0.1:9": 1}}],
        "routes": [
            {"id": "apex", "uri": "/api/mcp*", "host": LOCAL_DOMAIN, "priority": 20, "upstream_id": "closed"},
            {"id": "moved", "uri": "/api/mcp*", "host": LOCAL_DOMAIN, "priority": 30, "upstream_id": "closed",
             "vars": [NO_BEARER], "plugins": {"redirect": redirect}},
        ],
    }
    with pinned_apisix(tmp_path, yaml.safe_dump(rules) + "#END\n") as base:
        port = int(base.rsplit(":", 1)[1])
        status, location = ask(port, "/api/mcp/acme/open?a=1&b=two", {})
        assert status == 308 and location == f"https://{API_HOST}/api/mcp/acme/open?a=1&b=two"
        status, location = ask(port, "/api/mcp", {"Authorization": "Bearer x"})
        assert status == 502 and location is None


@requires_helmfile
def test_what_the_platform_hands_out_names_the_data_host(rendered):
    """T-4632: the addresses the assistant cites and the harvest writes into CKAN are on
    `api.{domain}`, so an anonymous reader is never sent through the apex's 308. The assistant
    is in the `dev` environment only."""
    docs = rendered("dev")
    env = {e["name"]: e.get("value") for d in docs if d.get("kind") == "Deployment"
           for c in d["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["JC_ASSISTANT_ENDPOINT_URL"] == "https://api.dev.joinedcontext.com"


@requires_helmfile
def test_the_named_mcp_servers_answer_a_platform_page_post_and_no_other_origin(apisix_config):
    # The Portal's /connect page posts `initialize` and `tools/list` from portal.{domain} (T-4949):
    # the preflight passes for the platform's own hosts only, with the domain's dots escaped.
    routes, plugins, _ = parsed_routes(apisix_config)
    for route_id in ("api-mcp-named", "context-mcp-named"):
        cors = routes[route_id]["plugins"]["cors"]
        assert cors["allow_methods"] == "POST,OPTIONS", route_id
        assert "Content-Type" in cors["allow_headers"].split(","), route_id
        assert cors.get("allow_origins", "") != "*" and "allow_credential" not in cors, route_id
        [pattern] = cors["allow_origins_by_regex"]
        assert pattern == "^https://[^@/]+\\." + LOCAL_DOMAIN.replace(".", "[.]") + "$", route_id
        assert routes[route_id]["vars"] == [NO_BEARER], route_id
    # The hub, which takes a bearer, keeps the chain's GET-only CORS.
    assert "cors" not in (routes["api-mcp-hub"].get("plugins") or {})
    assert plugins["context-endpoint"]["cors"]["allow_methods"] == "GET,HEAD,OPTIONS"
