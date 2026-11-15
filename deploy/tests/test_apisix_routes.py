"""Tests asserting APISIX standalone route table and plugin configurations render correctly."""

import base64
import json
import re
import shutil

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

# docs/Deployment/10-edge-routing-apisix.md section 2. The host constraint is part of
# the contract: an unbound `/*` route outranks the identity route on idm.{host}. The Portal
# lives on portal.{host} behind the edge login and the apex keeps the shared surfaces and
# redirects the rest (ADR-N-019).
LOCAL_DOMAIN = "joinedcontext.test"
PORTAL_HOST = f"portal.{LOCAL_DOMAIN}"
EXPECTED_ROUTES = {
    "portal-ui": {"uri": "/*", "priority": 1, "upstream_id": "portal-ui", "host": PORTAL_HOST},
    "portal-api": {"uri": "/api/v1/*", "priority": 10, "upstream_id": "portal-api", "host": PORTAL_HOST},
    "portal-metrics": {"uri": "/metrics", "priority": 5, "upstream_id": "portal-metrics", "host": PORTAL_HOST},
    "portal-redirect": {"uri": "/*", "priority": 1, "upstream_id": "portal-redirect", "host": LOCAL_DOMAIN},
    "context-space": {"uri": "/cs/*", "priority": 15, "upstream_id": "context-space", "host": LOCAL_DOMAIN},
    # T-2892: the reads of the space surface, with its chain and a bucket of their own.
    "context-space-read": {
        "uri": "/cs/*", "priority": 16, "upstream_id": "context-space-read", "host": LOCAL_DOMAIN,
    },
    "context-space-portal-read": {
        "uri": "/cs/*", "priority": 16, "upstream_id": "context-space-portal-read", "host": PORTAL_HOST,
    },
    "context-endpoint": {"uri": "/api/endpoint/*", "priority": 20, "upstream_id": "context-endpoint", "host": LOCAL_DOMAIN},
    # AP-108: the forge's container registry at the apex's root, where a node pulls App images.
    "gitea-registry": {"uri": "/v2/*", "priority": 10, "upstream_id": "gitea-registry", "host": LOCAL_DOMAIN},
    # containerd's OAuth POST to the token realm, which Gitea answers 404 so it falls back to GET.
    "gitea-registry-token": {
        "uri": "/v2/token", "priority": 11, "upstream_id": "gitea-registry-token", "host": LOCAL_DOMAIN,
    },
    "context-endpoint-portal": {
        "uri": "/api/endpoint/*", "priority": 20, "upstream_id": "context-endpoint-portal", "host": PORTAL_HOST,
    },
    # T-2726: the public catalogue page and the bundle it runs on, edge session optional (EP-81).
    "portal-public": {"uri": "/catalogue*", "priority": 5, "upstream_id": "portal-public", "host": PORTAL_HOST},
    "portal-public-assets": {
        "uri": "/assets/*", "priority": 5, "upstream_id": "portal-public-assets", "host": PORTAL_HOST,
    },
    # T-3108: a data view published as a public link, edge session optional.
    "portal-public-view": {"uri": "/v/*", "priority": 5, "upstream_id": "portal-public-view", "host": PORTAL_HOST},
    # T-3103: a form view published as a public form, edge session optional.
    "portal-public-form": {"uri": "/f/*", "priority": 5, "upstream_id": "portal-public-form", "host": PORTAL_HOST},
    # T-3265: a public Endpoint's API documentation, edge session optional.
    "portal-public-docs": {"uri": "/d/*", "priority": 5, "upstream_id": "portal-public-docs", "host": PORTAL_HOST},
    # T-4169: a public named MCP server's setup page, edge session optional.
    "portal-public-connect": {
        "uri": "/connect/*", "priority": 5, "upstream_id": "portal-public-connect", "host": PORTAL_HOST,
    },
    # T-5293: the glossary and the accessibility statement, exact paths, edge session optional.
    "portal-public-glossary": {
        "uri": "/glossary", "priority": 5, "upstream_id": "portal-public-glossary", "host": PORTAL_HOST,
    },
    "portal-public-accessibility": {
        "uri": "/accessibility", "priority": 5, "upstream_id": "portal-public-accessibility", "host": PORTAL_HOST,
    },
    # T-5627: the front door, exact path; the Portal sends a visitor without a session to /catalogue.
    "portal-home": {"uri": "/", "priority": 5, "upstream_id": "portal-home", "host": PORTAL_HOST},
    # T-3105: the gateway delivers the Portal's live-update notifications here, no login (API/01 §32).
    "portal-live-notify": {
        "uri": "/live-notify/*", "priority": 5, "upstream_id": "portal-live-notify", "host": PORTAL_HOST,
    },
    # T-2726: the organization's DCAT-AP feed for harvesters, anonymous (EP-84).
    "catalog-feed": {"uri": "/catalog*", "priority": 20, "upstream_id": "catalog-feed", "host": LOCAL_DOMAIN},
    # T-2490: the MCP hub over several Endpoints, bearer only (EP-87).
    "context-mcp-hub": {"uri": "/api/mcp*", "priority": 20, "upstream_id": "context-mcp-hub", "host": LOCAL_DOMAIN},
    "context-mcp-hub-portal": {
        "uri": "/api/mcp*", "priority": 20, "upstream_id": "context-mcp-hub-portal", "host": PORTAL_HOST,
    },
    # T-4141: the named MCP servers for a caller without a bearer, counted per client IP.
    "context-mcp-named": {
        "uri": "/api/mcp/*", "priority": 21, "upstream_id": "context-mcp-named", "host": LOCAL_DOMAIN,
    },
    "context-mcp-named-portal": {
        "uri": "/api/mcp/*", "priority": 21, "upstream_id": "context-mcp-named-portal", "host": PORTAL_HOST,
    },
}


def hosts_of(route):
    """A route binds one `host` or a `hosts` list; either way, the hosts it answers on."""
    return route.get("hosts") or [route.get("host")]
AUTHENTICATED_ROUTES = ("portal-api", "context-space", "context-endpoint")
# The routes the edge login sits on, and what each one does with a visitor who has no session
# (ADR-N-019 §3.3, AP-26, AP-28).
EDGE_LOGIN_ROUTES = {
    "portal-ui": {"unauth_action": "auth", "cookie_path": "/", "callback": f"https://{PORTAL_HOST}/callback"},
    "portal-api": {"unauth_action": "pass", "cookie_path": "/", "callback": f"https://{PORTAL_HOST}/callback"},
    # The public catalogue reads portal-ui's session when there is one and never redirects a
    # visitor (EP-81); `/assets/*` shares this config.
    "portal-public": {"unauth_action": "pass", "cookie_path": "/", "callback": f"https://{PORTAL_HOST}/callback"},
    # No route: the chain the Portal copies onto every published App's own host, with the App's
    # own client, callback and host-only cookie in place of these (ADR-N-037, AP-112).
    "apps-surface": {
        "unauth_action": "auth", "cookie_path": "/", "callback": f"https://{LOCAL_DOMAIN}/apps/callback",  # `/`: a `__Host-` cookie must be (T-4395)
    },
    # The endpoint surface on the Portal host: the session becomes the bearer the gateway
    # verifies, and a visitor without one is passed through as anonymous, never redirected. The
    # gateway reads the bearer only, so the userinfo header is not minted for it.
    "context-endpoint-portal": {
        "unauth_action": "pass", "cookie_path": "/", "callback": f"https://{PORTAL_HOST}/callback", "userinfo": False,
    },
    # And the space surface on the same host, for the same reason: the Portal UI reads a space
    # from its own origin (`sdk/src/grid/source.ts`), so the session has to become the bearer
    # there too. Anonymous is passed through, never redirected (T-2454).
    "context-space-portal": {
        "unauth_action": "pass", "cookie_path": "/", "callback": f"https://{PORTAL_HOST}/callback", "userinfo": False,
    },
}


@pytest.fixture(scope="module")
def apisix_config(rendered):
    docs = rendered("local")
    cm = next(
        (d for d in docs if d.get("kind") == "ConfigMap" and d.get("metadata", {}).get("name") == "apisix-standalone-base"),
        None,
    )
    assert cm is not None, "apisix-standalone-base ConfigMap not found in rendered output"
    raw = cm["data"]["apisix.yaml"]
    return raw, yaml.safe_load(raw)


@requires_helmfile
def test_expected_routes_exist_with_properties(apisix_config):
    _, parsed = apisix_config
    routes_by_id = {r["id"]: r for r in parsed.get("routes", [])}
    for route_id, expected in EXPECTED_ROUTES.items():
        assert route_id in routes_by_id, f"Route {route_id} missing from apisix.yaml"
        route = routes_by_id[route_id]
        assert route.get("uri") == expected["uri"]
        assert route.get("priority") == expected["priority"]
        assert route.get("upstream_id") == expected["upstream_id"]
        assert hosts_of(route) == hosts_of(expected)


@requires_helmfile
def test_only_the_endpoint_surface_passes_a_websocket_upgrade(apisix_config):
    """T-3905: `{endpoint}/ws` reaches the gateway, which verifies the bearer before it answers 101
    (platform T-3885); every other route keeps APISIX's default and drops the upgrade."""
    _, parsed = apisix_config
    upgraded = sorted(r["id"] for r in parsed.get("routes", []) if r.get("enable_websocket"))
    # T-4630: and the same surface on the data host api.{domain}. T-4873: on `-ws` routes of their
    # own that take `{endpoint}/ws` alone, so the open-socket limit applies to the socket only;
    # T-4903: and only they keep the 300 s read timeout, a plain read never gets it.
    assert upgraded == ["api-context-endpoint-ws", "context-endpoint-portal-ws", "context-endpoint-ws"]
    routes = {r["id"]: r for r in parsed.get("routes", [])}
    for route_id in upgraded:
        route, parent = routes[route_id], routes[route_id.removesuffix("-ws")]
        assert route["vars"] == [["uri", "~~", "^/api/endpoint/[^/]+/ws$"]], route_id
        assert route["uri"] == parent["uri"] and route["priority"] > parent["priority"], route_id
        assert route.get("plugin_config_id") == parent.get("plugin_config_id"), route_id


@requires_helmfile
def test_routes_have_plugin_config_with_request_id(apisix_config):
    _, parsed = apisix_config
    routes_by_id = {r["id"]: r for r in parsed.get("routes", [])}
    plugin_configs = {pc["id"]: pc.get("plugins", {}) for pc in parsed.get("plugin_configs", [])}
    for route_id in EXPECTED_ROUTES:
        route = routes_by_id[route_id]
        plugin_config_id = route.get("plugin_config_id")
        assert plugin_config_id is not None, f"Route {route_id} missing plugin_config_id"
        assert plugin_config_id in plugin_configs, f"Plugin config {plugin_config_id} not found"
        plugins = plugin_configs[plugin_config_id]
        assert plugins.get("request-id", {}).get("include_in_response") is True


def realm_of(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return json.loads(base64.b64decode(next(iter(doc["data"].values()))))
    pytest.fail("no keycloak-config-cli realm Secret in the local render")


@requires_helmfile
def test_the_edge_login_sits_on_the_portal_and_apps_routes_only(apisix_config):
    """ADR-N-019: the openid-connect plugin in session mode is the one login front, on the
    Portal routes and the apps surface, with the confidential `edge` client; no other route
    carries it (the context surfaces and the forge stay bearer-only or anonymous)."""
    _, parsed = apisix_config
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    carrying = {config_id for config_id, p in plugins.items() if "openid-connect" in p}
    assert carrying == set(EDGE_LOGIN_ROUTES), carrying
    for route in parsed["routes"]:
        assert "openid-connect" not in route.get("plugins", {}), route["id"]
    for config_id, expected in EDGE_LOGIN_ROUTES.items():
        oidc = plugins[config_id]["openid-connect"]
        assert oidc["client_id"] == "edge", config_id
        assert oidc["client_secret"] == "${{EDGE_CLIENT_SECRET}}", "the secret is an env var from a Secret (AP-27)"
        assert oidc["discovery"] == (
            f"https://idm.{LOCAL_DOMAIN}/realms/local/.well-known/openid-configuration"
        ), config_id
        assert oidc["bearer_only"] is False, "session mode"
        assert oidc["unauth_action"] == expected["unauth_action"], config_id
        assert oidc["redirect_uri"] == expected["callback"], "the callback is under the route it belongs to"
        assert oidc["use_pkce"] is True, "the realm demands S256 from every browser client"
        assert oidc["ssl_verify"] is True, config_id
        assert oidc["set_userinfo_header"] is expected.get("userinfo", True), config_id
        assert oidc["set_access_token_header"] is True, config_id
        assert oidc["set_id_token_header"] is False and oidc["set_refresh_token_header"] is False, config_id
        assert oidc["session"]["secret"] == "${{OIDC_SESSION_SECRET}}", config_id
        assert oidc["session"]["cookie_path"] == expected["cookie_path"], config_id


def unauth_action_on_portal_host(parsed, path):
    """What the edge does with a visitor without a session on `portal.{host}{path}`: the
    highest-priority route matching the path (an exact `uri`, or a prefix before `*`)."""
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    matching = [
        route for route in parsed["routes"]
        if PORTAL_HOST in hosts_of(route) and not route.get("vars") and (
            path.startswith(route["uri"][:-1]) if route["uri"].endswith("*") else path == route["uri"]
        )
    ]
    route = max(matching, key=lambda r: r["priority"])
    return route["id"], plugins[route["plugin_config_id"]]["openid-connect"]["unauth_action"]


@requires_helmfile
@pytest.mark.parametrize("path", ["/", "/glossary", "/accessibility", "/catalogue", "/catalogue/air-quality"])
def test_the_public_pages_pass_a_visitor_without_a_session(apisix_config, path):
    """T-5293: every Term's link and the public footer open /glossary and /accessibility, which the
    Portal draws for a visitor; the edge must not send that visitor to Keycloak first."""
    _, parsed = apisix_config
    assert unauth_action_on_portal_host(parsed, path)[1] == "pass", path


@requires_helmfile
@pytest.mark.parametrize("path", [
    "/projects/demo/home", "/home", "/projects", "/glossary/x", "/glossaryx", "/accessibility-admin",
    "/docs/API/07-websocket", "/docs/User-Guide/01-getting-started",
])
def test_every_other_portal_page_keeps_the_login(apisix_config, path):
    """T-5293: the public pages are exact paths; a private page beside them still logs in first.
    T-6200: the documentation under /docs is for signed-in people only."""
    _, parsed = apisix_config
    assert unauth_action_on_portal_host(parsed, path) == ("portal-ui", "auth"), path


@requires_helmfile
def test_a_presented_bearer_is_never_verified_at_the_edge(apisix_config):
    """T-0252 still holds: the realm signs ES256 and lua-resty-openidc verifies RS/HS only. With
    `use_jwks`, a `public_key` or an `introspection_endpoint` the plugin would verify a bearer
    it finds and answer 401 before `unauth_action` is consulted, and every CLI call would die at
    the edge. The upstream verifies bearer tokens (docs Deployment/10 §4, OPS-33)."""
    _, parsed = apisix_config
    for pc in parsed["plugin_configs"]:
        oidc = pc.get("plugins", {}).get("openid-connect")
        if not oidc:
            continue
        assert oidc.get("use_jwks", False) is False, pc["id"]
        assert "public_key" not in oidc and "introspection_endpoint" not in oidc, pc["id"]


@requires_helmfile
def test_the_session_cookie_is_scoped_and_bounded_by_the_realm(apisix_config, rendered):
    """AP-29: `Secure`, `HttpOnly`, `SameSite=Lax`, host-only, a name per surface so a browser
    never presents one surface's session to another, and a lifetime that
    never outlives the realm's SSO idle time or max lifespan. The keys are the flat
    lua-resty-session 4 ones APISIX 3.17 validates: a nested `cookie.path` is accepted and
    ignored, and the cookie would land on `/`."""
    _, parsed = apisix_config
    realm = realm_of(rendered)
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    names = {}
    for config_id in EDGE_LOGIN_ROUTES:
        session = plugins[config_id]["openid-connect"]["session"]
        assert "cookie" not in session, f"{config_id}: nested lua-resty-session 3 keys are ignored by APISIX 3.17"
        assert session["cookie_secure"] is True and session["cookie_http_only"] is True, config_id
        assert session["cookie_same_site"] == "Lax", config_id
        assert "cookie_domain" not in session, f"{config_id}: the cookie must not leak across hosts"
        assert session["idling_timeout"] <= realm["ssoSessionIdleTimeout"], config_id
        assert session["rolling_timeout"] <= realm["ssoSessionIdleTimeout"], config_id
        assert session["absolute_timeout"] <= realm["ssoSessionMaxLifespan"], config_id
        names[config_id] = session["cookie_name"]
    # The two Portal routes share one session; the apps surface has its own.
    assert names["portal-ui"] == names["portal-api"]
    assert names["apps-surface"] != names["portal-ui"]


@requires_helmfile
def test_every_edge_session_cookie_is_host_prefixed(apisix_config):
    """T-4395, AP-29: an App may set a cookie for the parent domain. The browser keeps a
    `__Host-` cookie only when it is `Secure`, on `Path=/` and has no `Domain`, so no App plants
    one of the same name beside the edge's session. Every `openid-connect` session, not only the
    login routes."""
    _, parsed = apisix_config
    sessions = {
        pc["id"]: pc["plugins"]["openid-connect"]["session"]
        for pc in parsed["plugin_configs"]
        if "session" in pc.get("plugins", {}).get("openid-connect", {})
    }
    assert sessions, "no edge session rendered"
    for config_id, session in sessions.items():
        assert session["cookie_name"].startswith("__Host-"), config_id
        assert session["cookie_path"] == "/", config_id
        assert session["cookie_secure"] is True, config_id
        assert "cookie_domain" not in session, config_id


@requires_helmfile
def test_the_apex_redirects_to_the_portal_below_every_other_apex_route(apisix_config):
    """ADR-N-019 §3.1: the apex answers `/` with a redirect to the Portal host, keeping the path
    so links that predate the move still open the same page. It is the lowest-priority apex
    route, so the shared surfaces (/apps, /git, /cs, /api/endpoint) are never shadowed."""
    _, parsed = apisix_config
    routes = {r["id"]: r for r in parsed["routes"]}
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    redirect = plugins["portal-redirect"]["redirect"]
    assert redirect == {"uri": f"https://{PORTAL_HOST}$uri", "append_query_string": True, "ret_code": 302}
    assert "openid-connect" not in plugins["portal-redirect"]
    apex = {r_id: r for r_id, r in routes.items() if LOCAL_DOMAIN in hosts_of(r)}
    assert "portal-redirect" in apex
    for r_id, route in apex.items():
        if r_id != "portal-redirect":
            assert route["priority"] > routes["portal-redirect"]["priority"], r_id
    # Nothing on the apex proxies the Portal any more: no App is served there (ADR-N-037).
    portal_upstreams = {
        u["id"] for u in parsed["upstreams"] if any(n.startswith("portal.") for n in u["nodes"])
    }
    assert {r_id for r_id in apex if r_id in portal_upstreams} == {"portal-redirect"}


@requires_helmfile
def test_the_endpoint_surface_answers_on_the_portal_host_too(apisix_config):
    """The Portal UI builds endpoint links and its access fetches from its own origin, so
    `portal.{host}/api/endpoint/*` must reach the gateway instead of falling through to
    portal-ui's `/*`, which answers with the edge login and then the Portal's 404 (EP-01,
    EP-27). A second route serves the Portal host, same upstream and priority, with the edge
    session turned into the bearer (ADR-N-019); the apex route stays anonymous-or-bearer."""
    _, parsed = apisix_config
    routes = {r["id"]: r for r in parsed["routes"]}
    apex, portal = routes["context-endpoint"], routes["context-endpoint-portal"]
    assert hosts_of(apex) == [LOCAL_DOMAIN] and hosts_of(portal) == [PORTAL_HOST]
    for endpoint in (apex, portal):
        assert endpoint["uri"] == "/api/endpoint/*"
        assert endpoint["priority"] > routes["portal-ui"]["priority"]
        upstream = next(u for u in parsed["upstreams"] if u["id"] == endpoint["upstream_id"])
        assert all(node.startswith("context-gateway.") for node in upstream["nodes"])
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    assert "openid-connect" not in plugins[apex["plugin_config_id"]]
    assert plugins[portal["plugin_config_id"]]["openid-connect"]["unauth_action"] == "pass"


@requires_helmfile
def test_no_route_of_the_base_serves_an_app(apisix_config):
    """ADR-N-037, AP-133, T-2839: every App is served on its own host `{name}.apps.{domain}`,
    whose routes the Portal composes, so no route helm renders matches `/apps/`, on the apex or
    anywhere else; the old path's 308 to the App's host is the Portal's too. The `apps-surface`
    plugin config stays, with no route: it is the chain the Portal copies onto each App's."""
    _, parsed = apisix_config
    for route in parsed["routes"]:
        text = json.dumps(route)
        assert "/apps" not in text, route["id"]
        assert ".apps." not in text, route["id"]
    assert all(route.get("plugin_config_id") != "apps-surface" for route in parsed["routes"])
    template = next(pc for pc in parsed["plugin_configs"] if pc["id"] == "apps-surface")["plugins"]
    for plugin in ("openid-connect", "serverless-pre-function", "limit-count", "response-rewrite"):
        assert plugin in template, plugin


@requires_helmfile
def test_apisix_yaml_ends_with_end_marker_and_has_no_raw_tokens(apisix_config):
    raw, _ = apisix_config
    lines = [line.strip() for line in raw.strip().splitlines() if line.strip()]
    assert lines[-1] == "#END", "apisix.yaml must end with exact #END marker"
    assert "${DOMAIN}" not in raw, "Unsubstituted ${DOMAIN} found in apisix.yaml"
    assert "${REALM}" not in raw, "Unsubstituted ${REALM} found in apisix.yaml"


# --- T-0025: the Linkerd Server scoping the public data-plane port -----------------


def servers(docs: list[dict]) -> dict[str, dict]:
    return {
        d["metadata"]["name"]: d
        for d in docs
        if d.get("kind") == "Server" and d.get("apiVersion") == "policy.linkerd.io/v1beta3"
    }


@requires_helmfile
def test_data_plane_port_is_scoped_by_a_linkerd_server(rendered):
    """Without a Server on 9080 the namespace's mandatory-mTLS default policy governs the
    public port, and every request from an unmeshed ingress controller is refused."""
    server = servers(rendered("production"))["apisix-configuration-apisix-gateway"]
    assert server["spec"]["port"] == 9080
    assert server["spec"]["podSelector"]["matchLabels"]["app.kubernetes.io/name"] == "apisix"
    assert server["spec"]["proxyProtocol"] == "HTTP/1"


@requires_helmfile
def test_access_policy_follows_whether_the_ingress_edge_is_meshed(rendered):
    """`allowUnauthenticatedIngress` is the only thing that may open the port to plaintext,
    and only 9080: every other APISIX port stays on the namespace default."""
    production = servers(rendered("production"))["apisix-configuration-apisix-gateway"]
    assert production["spec"]["accessPolicy"] == "all-authenticated"

    # dev runs behind the kit's Traefik, which has no mesh identity.
    dev = servers(rendered("dev"))["apisix-configuration-apisix-gateway"]
    assert dev["spec"]["accessPolicy"] == "all-unauthenticated"

    ports = {s["spec"]["port"] for s in servers(rendered("production")).values()}
    assert 9091 not in ports and 9092 not in ports and 9180 not in ports


def selects(policy: dict, labels: dict) -> bool:
    match = policy["spec"]["podSelector"].get("matchLabels", {})
    return bool(match) and all(labels.get(k) == v for k, v in match.items())


@requires_helmfile
def test_every_route_upstream_is_a_pod_apisix_is_allowed_to_reach(rendered):
    """A route the egress policy does not name resolves and is then dropped on the way out.

    APISIX runs under a default-deny egress, so each upstream in the standalone rule file
    needs a NetworkPolicy line naming that pod and that port — and, on a meshed cluster, the
    peer's inbound proxy port too, because that is where the outbound proxy actually connects.
    Neither is visible in a render of the route table alone: the edge answers 504 with the
    upstream 2/2 and healthy, which reads as an application fault and is not one.
    """
    docs = rendered("dev")
    config = yaml.safe_load(next(
        d for d in docs
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )["data"]["apisix.yaml"])

    apisix_labels = {
        "app.kubernetes.io/name": "apisix",
        "app.kubernetes.io/instance": "apisix-apisix",
    }
    # A peer selector is a subset of the pod's labels, not the whole set: the policy that
    # names only `app.kubernetes.io/name` still selects a pod the Service picks by two labels.
    # A peer selecting by matchExpressions alone (the ingress controller of the issuer hop) is
    # no route upstream and is left out: read as an empty label set it would match every pod.
    allowed = [
        (frozenset(peer["podSelector"]["matchLabels"].items()), port["port"])
        for policy in docs
        if policy.get("kind") == "NetworkPolicy" and selects(policy, apisix_labels)
        for rule in policy["spec"].get("egress", [])
        for peer in rule.get("to", []) if peer.get("podSelector", {}).get("matchLabels")
        for port in rule.get("ports", [])
    ]
    services = {
        (d["metadata"]["name"], d["metadata"]["namespace"]): d
        for d in docs if d.get("kind") == "Service"
    }
    # A NetworkPolicy port is the port on the wire, and kube-proxy has already rewritten the
    # Service port to the target one by then. `targetPort` is usually a name, so it is
    # resolved against the container that carries it rather than assumed equal to the port.
    container_ports = {
        (port["name"], frozenset(workload["spec"]["selector"]["matchLabels"].items())): port["containerPort"]
        for workload in docs if workload.get("kind") in ("Deployment", "StatefulSet")
        for container in workload["spec"]["template"]["spec"]["containers"]
        for port in container.get("ports", []) if "name" in port
    }

    for upstream in config.get("upstreams", []):
        for node in upstream["nodes"]:
            host, _, port = node.rpartition(":")
            if not host.endswith(".svc.cluster.local"):
                continue
            name, namespace = host.split(".")[:2]
            service = services.get((name, namespace))
            assert service, f"upstream {host} names no Service in the render"
            selector = frozenset(service["spec"]["selector"].items())
            target = next(
                p.get("targetPort", p["port"]) for p in service["spec"]["ports"]
                if p["port"] == int(port)
            )
            if isinstance(target, str):
                target = container_ports[(target, selector)]
            for allowed_port, why in ((target, "the upstream port"), (4143, "the peer's inbound proxy")):
                assert any(
                    peer <= selector and port_of == allowed_port for peer, port_of in allowed
                ), (
                    f"apisix has no egress rule to {name} on {allowed_port} ({why}); "
                    f"route upstream {node} will time out at the edge"
                )


@requires_helmfile
def test_no_route_carries_a_delivery_path_guard_any_more(apisix_config):
    """T-3925: the gateway serves no token-less delivery path since the broker delivers itself
    (plan 5.5), so no route keeps a guard for one."""
    _, parsed = apisix_config
    for config_id in ("context-endpoint", "context-space"):
        config = next(p for p in parsed["plugin_configs"] if p["id"] == config_id)
        assert "egress" not in config["plugins"]["serverless-pre-function"]["functions"][0]


@requires_helmfile
def test_the_portal_scrape_path_is_refused_at_the_edge(apisix_config):
    """OPS-16: the Portal serves /metrics on the port APISIX publishes it on, so the edge is
    what keeps the series inside the cluster. The route terminates at APISIX and never dials
    the upstream; the ServiceMonitor reaches the pod directly and does not pass through here.
    """
    _, parsed = apisix_config
    routes = {r["id"]: r for r in parsed.get("routes", [])}
    plugins = {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}
    abort = plugins["portal-metrics"]["fault-injection"]["abort"]
    assert abort["http_status"] == 404, abort
    # Above the UI's catch-all, or `/*` answers first and proxies the scrape to the Portal.
    assert routes["portal-metrics"]["priority"] > routes["portal-ui"]["priority"]
