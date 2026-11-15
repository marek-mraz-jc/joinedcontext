"""Tests for the APISIX plugin chain and the public edge TLS configuration.

docs/Deployment/10-edge-routing-apisix.md section 4 (plugin chain) and
docs/Deployment/08-security-hardening.md section 2, layer 9 (edge TLS).
"""

import re
from pathlib import Path

import pytest
import yaml

# Headers a client must never be able to set: each one is either a tenancy or authorization
# claim the platform trusts internally, or a proxy hint APISIX sets itself. One list, shared
# with the walk over the component sources: two copies of it drifted once already (T-1672).
from conftest import set_global
from test_edge_attack_surface import TRUSTED_HEADERS as FORGED_HEADERS

UI_CONFIGS = ("portal-ui", "portal-public", "apps-surface", "keycloak", "gitea-forge")
# The routes the Portal itself answers; their headers are the Portal's own (T-1732).
PORTAL_CONFIGS = (
    "portal-ui",
    "portal-public",
    "portal-public-form",
    "portal-api",
    "portal-well-known",
    "portal-live-notify",
    "portal-metrics",
    "apps-surface",
    "portal-redirect",
)
# Pages and app bundles a browser may cache; every other route is `no-store`.
CACHEABLE_CONFIGS = ("portal-ui", "portal-public", "portal-public-form", "apps-surface")
# A published form's page, which the Portal lets the sites its Endpoint names frame through the
# page's own `frame-ancestors`; X-Frame-Options cannot name a site, so the edge sends none there
# (EP-101, T-3266). Every other route keeps SAMEORIGIN or DENY.
FRAMED_BY_PAGE = ("portal-public-form",)
API_CONFIGS = ("portal-api", "context-space", "context-endpoint")
GATEWAY_UPSTREAMS = ("context-space", "context-endpoint")
# The gateway answers these too, and sets their Cache-Control itself (EP-84: the DCAT-AP feed).
GATEWAY_ANSWERED = (*GATEWAY_UPSTREAMS, "catalog-feed")
# BSI TR-02102-2: no CBC, no 3DES, no static RSA key exchange, no anonymous suites.
FORBIDDEN_CIPHER_MARKERS = ("_CBC_", "3DES", "TLS_RSA_", "_anon_", "RC4")


@pytest.fixture(scope="module")
def plugin_configs(rendered):
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    parsed = yaml.safe_load(cm["data"]["apisix.yaml"])
    return {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}


@pytest.fixture(scope="module")
def upstreams(rendered):
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    parsed = yaml.safe_load(cm["data"]["apisix.yaml"])
    return {u["id"]: u for u in parsed["upstreams"]}


def test_every_route_clears_every_forged_header(plugin_configs):
    """T-0026: header sanitization runs on every route, not only the authenticated ones."""
    for config_id, plugins in plugin_configs.items():
        sanitizer = plugins.get("serverless-pre-function")
        assert sanitizer, f"{config_id} has no serverless-pre-function"
        assert sanitizer["phase"] == "rewrite"
        lua = "\n".join(sanitizer["functions"])
        for header in FORGED_HEADERS:
            assert header in lua, f"{config_id} does not clear {header}"
        assert "ngx.req.clear_header" in lua


def test_x_forwarded_for_survives_sanitization(plugin_configs):
    """It is the client-IP audit trail and the rate-limit key; nginx maintains it itself."""
    for config_id, plugins in plugin_configs.items():
        lua = "\n".join(plugins["serverless-pre-function"]["functions"])
        assert '"X-Forwarded-For"' not in lua, f"{config_id} clears X-Forwarded-For"


def test_the_trace_context_crosses_the_edge(plugin_configs):
    """OPS-17: APISIX neither clears nor rewrites `traceparent`/`tracestate`, so a trace a client
    starts reaches the gateway, which relays it to the broker (platform
    `space_surface_tests::the_trace_context_reaches_the_broker_unchanged`)."""
    for config_id, plugins in plugin_configs.items():
        lua = "\n".join(plugins["serverless-pre-function"]["functions"]).lower()
        rewrite = plugins.get("proxy-rewrite", {}).get("headers", {})
        touched = [
            name.lower()
            for section in ("set", "add", "remove")
            for name in (rewrite.get(section) or [])
        ]
        for header in ("traceparent", "tracestate"):
            assert f'"{header}"' not in lua, f"{config_id} clears {header}"
            assert header not in touched, f"{config_id} rewrites {header}"


def test_security_response_headers_on_every_route(plugin_configs):
    """T-0028, OPS-34: HSTS, nosniff, referrer policy everywhere; framing and caching by route class."""
    for config_id, plugins in plugin_configs.items():
        headers = plugins["response-rewrite"]["headers"]["set"]
        assert headers["Strict-Transport-Security"] == "max-age=31536000; includeSubDomains; preload"
        assert headers["X-Content-Type-Options"] == "nosniff"
        # The Portal answers `no-referrer` itself, and the edge sets the header on every
        # response including the ones it writes (the login redirect), so the Portal's own routes
        # say the same thing on both sides and no request carries a Portal path to another
        # origin. The upstreams that are somebody else's application keep the wider value: Gitea
        # and Keycloak check the `Referer` on some form posts, and taking it away breaks a login.
        # security.txt is answered by the edge itself (T-1721): no upstream reads a `Referer`.
        narrow = PORTAL_CONFIGS + ("security-txt",)
        expected_referrer = "no-referrer" if config_id in narrow else "strict-origin-when-cross-origin"
        assert headers["Referrer-Policy"] == expected_referrer, config_id
        expected_frame = None if config_id in FRAMED_BY_PAGE else "SAMEORIGIN" if config_id in UI_CONFIGS else "DENY"
        assert headers.get("X-Frame-Options") == expected_frame, config_id
        if config_id.removesuffix("-portal").removesuffix("-apps") in GATEWAY_ANSWERED:
            # T-2262, EP-51: the gateway sets Cache-Control on every answer itself (`private`,
            # `no-store`, or `no-cache` with an ETag on a schema artifact); the edge leaves it be.
            assert "Cache-Control" not in headers, config_id
        elif config_id not in CACHEABLE_CONFIGS:
            assert headers["Cache-Control"] == "no-store, no-cache, must-revalidate", config_id


def test_rate_limit_classes(plugin_configs):
    """T-0029: one class per route, keyed on the strongest identity that route carries."""
    for config_id in ("portal-ui", "apps-surface", "portal-redirect"):
        ui = plugin_configs[config_id]["limit-count"]
        assert (ui["count"], ui["time_window"], ui["key"]) == (300, 60, "remote_addr"), config_id

    for config_id in ("portal-api", "context-space"):
        api = plugin_configs[config_id]["limit-count"]
        assert (api["count"], api["time_window"]) == (1200, 60), config_id
        assert "authorization" in api["key"], config_id
    # T-2669: a browser carries the edge session, which openid-connect turns into X-Access-Token;
    # keyed on the bearer alone, every session behind one address shared one bucket.
    portal = plugin_configs["portal-api"]["limit-count"]
    assert portal["key_type"] == "var_combination"
    assert portal["key"].split() == ["$http_authorization", "$http_x_access_token", "$remote_addr"]
    assert "X-Access-Token" in plugin_configs["portal-api"]["serverless-pre-function"]["functions"][0]

    # T-4717: the data class is 10 a second per address and bearer, now that the edge sees the
    # real address; a bot above it gets 429 alone.
    for config_id in ("context-endpoint", "context-endpoint-portal"):
        stream = plugin_configs[config_id]["limit-count"]
        assert (stream["count"], stream["time_window"]) == (600, 60), config_id
        # T-4714: a limit-conn on the whole surface held every WebSocket and quick read to 10.
        assert "limit-conn" not in plugin_configs[config_id], config_id
    routes = yaml.safe_load((Path(__file__).resolve().parent.parent / "components/context-gateway/apisix-routes.yaml").read_text())
    for parent in ("context-endpoint", "api-context-endpoint", "context-endpoint-portal"):
        route, base = routes[f"{parent}-bulk"], routes[parent]
        assert route.get("pluginConfig") == base.get("pluginConfig", parent)
        assert (route.get("host"), route.get("subDomain")) == (base.get("host"), base.get("subDomain"))
        assert route["uri"] == base["uri"] and route["priority"] > base["priority"]
        bulk = route["plugins"]["limit-conn"]
        assert bulk["conn"] == 10
        # The endpoint surface passes anonymous requests through, so the bucket cannot be keyed
        # on the bearer token alone — every anonymous caller would share one.
        assert bulk["key_type"] == "var_combination"
        assert "remote_addr" in bulk["key"]
        pattern = re.compile(route["vars"][0][2])
        for path in ("/api/endpoint/abc/file.csv", "/api/endpoint/abc/file.geojson",
                     "/api/endpoint/abc/ngsi-ld/v1/temporal/entities"):
            assert pattern.search(path), (parent, path)
        for path in ("/api/endpoint/abc/ws", "/api/endpoint/abc/ngsi-ld/v1/entities",
                     "/api/endpoint/abc/odata/v4/Thing", "/api/endpoint/file.csv/mcp"):
            assert not pattern.search(path), (parent, path)


# ADR-N-035: each chain the Organization tunes, its class, and the catalog default helm renders.
RATE_CLASSES = {
    "portal-ui": ("web", 300),
    "portal-public": ("web", 300),
    "portal-public-form": ("web", 300),
    "apps-surface": ("web", 300),
    "portal-redirect": ("web", 300),
    "portal-api": ("api", 1200),
    "context-endpoint": ("publicEndpoint", 600),
    "context-endpoint-portal": ("publicEndpoint", 600),
    "context-space": ("dataWrite", 1200),
    "context-space-portal": ("dataWrite", 1200),
}
# The routes that count a class apart from their shared chain, with the route-level limit.
ROUTE_RATE_CLASSES = {
    "context-space-read": ("dataRead", 1200, "context-space"),
    "context-space-portal-read": ("dataRead", 1200, "context-space-portal"),
    "api-context-space-read": ("dataRead", 1200, "api-context-space"),
}


def test_the_organization_tunes_each_class_by_its_label(rendered):
    """T-2892: the Portal replaces the count of a chain labelled `jc-rate-class` with the
    Organization's rate; helm renders the default, and every other chain keeps its own."""
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    configs = yaml.safe_load(cm["data"]["apisix.yaml"])["plugin_configs"]
    labelled = {
        pc["id"]: (pc["labels"]["jc-rate-class"], pc["plugins"]["limit-count"]["count"])
        for pc in configs
        if "jc-rate-class" in (pc.get("labels") or {})
    }
    assert labelled == RATE_CLASSES
    for pc in configs:
        assert "labels" not in pc or pc["id"] in RATE_CLASSES, pc["id"]


# The named MCP servers' anonymous callers, per client IP a minute (T-4141).
ANONYMOUS_ROUTE_LIMITS = {"context-mcp-named": 60, "context-mcp-named-portal": 60, "api-mcp-named": 60}


def test_space_reads_and_writes_count_apart(rendered):
    """T-2892, ADR-N-035: GET and HEAD on /cs/* win over the write route by priority and method,
    share its chain, and replace only its limit-count, so dataRead and dataWrite are two buckets."""
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    routes = {r["id"]: r for r in yaml.safe_load(cm["data"]["apisix.yaml"])["routes"]}
    for route_id, (rate_class, count, chain) in ROUTE_RATE_CLASSES.items():
        read, write = routes[route_id], routes[chain]
        assert read["labels"] == {"jc-rate-class": rate_class}, route_id
        assert read["plugin_config_id"] == write["plugin_config_id"], route_id
        assert read["methods"] == ["GET", "HEAD"] and "methods" not in write, route_id
        assert (read["uri"], read.get("host")) == (write["uri"], write.get("host")), route_id
        assert read["priority"] > write["priority"], route_id
        assert set(read["plugins"]) == {"limit-count"}, route_id
        limit = read["plugins"]["limit-count"]
        assert (limit["count"], limit["key"], limit["rejected_code"]) == (count, "$http_authorization $remote_addr", 429)
    for route in routes.values():
        if route["id"] in ANONYMOUS_ROUTE_LIMITS:
            # A fixed per-IP count for callers without a bearer (T-4141): no rate class, so the
            # Portal never rewrites it, and nothing beside the count; the named MCP servers also
            # answer the platform's own pages cross-origin (T-4949).
            cors = {"cors"} if route["id"] in ("context-mcp-named", "api-mcp-named") else set()
            assert "labels" not in route and set(route["plugins"]) == {"limit-count"} | cors, route["id"]
            limit = route["plugins"]["limit-count"]
            assert (limit["count"], limit["key"]) == (ANONYMOUS_ROUTE_LIMITS[route["id"]], "remote_addr")
            assert route["vars"] == [["http_authorization", "!", "~*", "^bearer\\s+\\S"]], route["id"]
        elif route["id"] == "ckan-webassets":
            # T-4201: it keeps the config's CSP baseline and adds the year-long cache of a
            # content-hashed file; T-4717: and counts its assets in a bucket of their own
            # (tests/test_edge_asset_cache.py).
            assert "labels" not in route and set(route["plugins"]) == {"serverless-post-function", "limit-count"}, route["id"]
        elif route["id"].endswith("context-endpoint-bulk") or route["id"] == "context-endpoint-portal-bulk":
            # T-4717: the bulk exports add their concurrency limit to the surface's chain.
            assert "labels" not in route and set(route["plugins"]) == {"limit-conn"}, route["id"]
        elif route["id"].endswith("-ws") and route["id"].startswith(("context-endpoint", "api-context-endpoint")):
            # T-4873: the Endpoint WebSockets bound open sockets per client address at the edge.
            assert "labels" not in route and set(route["plugins"]) == {"limit-conn"}, route["id"]
            limit = route["plugins"]["limit-conn"]
            assert (limit["conn"], limit["burst"], limit["key"]) == (50, 0, "$remote_addr"), route["id"]
            assert route["enable_websocket"] is True, route["id"]
        elif route["id"] == "keycloak-discovery":
            # T-5611: the realm's discovery documents drop registration_endpoint and keep the
            # config's headers (tests/test_keycloak_discovery.py).
            assert "labels" not in route and set(route["plugins"]) == {"response-rewrite"}, route["id"]
        elif route["id"] == "api-context-endpoint-history":
            # T-6185: the data host's history keeps the bulk limit and the reads' edge cache.
            assert "labels" not in route and set(route["plugins"]) == {"limit-conn", "proxy-buffering", "proxy-cache"}, route["id"]
        elif route["id"] == "api-context-endpoint":
            # T-5900: the data host keeps the edge cache of anonymous reads (tests/test_edge_api_cache.py).
            # Buffered, or proxy-cache stores nothing (T-6185).
            assert "labels" not in route and set(route["plugins"]) == {"proxy-buffering", "proxy-cache"}, route["id"]
        elif route["id"] == "portal-public-assets":
            # T-6539: the Portal's bundle counts in a bucket of its own, as CKAN's assets
            # (tests/test_edge_asset_cache.py).
            assert "labels" not in route and set(route["plugins"]) == {"limit-count"}, route["id"]
        elif route["id"] == "api-oauth":
            # T-4630, ADR-N-052 §4: minting an anonymous token counts per client address.
            assert "labels" not in route and set(route["plugins"]) == {"limit-count"}, route["id"]
        elif route["id"] not in ROUTE_RATE_CLASSES:
            assert "labels" not in route and "plugins" not in route, route["id"]


def test_every_rate_limit_rejects_with_429(plugin_configs):
    for config_id, plugins in plugin_configs.items():
        for plugin in ("limit-count", "limit-conn"):
            if plugin in plugins:
                assert plugins[plugin]["rejected_code"] == 429, f"{config_id}/{plugin}"


def test_gateway_upstreams_carry_streaming_timeouts(upstreams):
    """T-0030: five minutes to read, and a pool sized for long-lived connections. T-4903: on the
    Endpoint surface only the WebSocket and the bulk exports keep five minutes; the plain route,
    whose reads the gateway ends at 10 s, reads for 60 s."""
    reads = {"context-space": 300, "context-endpoint": 60, "context-endpoint-ws": 300,
             "context-endpoint-bulk": 300, "api-context-endpoint": 60, "api-context-endpoint-ws": 300,
             "context-endpoint-portal": 60, "context-endpoint-portal-ws": 300}
    for upstream_id, read in reads.items():
        upstream = upstreams[upstream_id]
        assert upstream["timeout"] == {"connect": 6, "send": 60, "read": read}, upstream_id
        if upstream_id != "context-endpoint-bulk":
            assert upstream["keepalive_pool"]["size"] == 320, upstream_id


def test_gateway_routes_stream_unbuffered(plugin_configs):
    """A 300 s read timeout is pointless if nginx buffers the whole response first."""
    for config_id in GATEWAY_UPSTREAMS:
        assert plugin_configs[config_id]["proxy-buffering"]["disable_proxy_buffering"] is True


def test_endpoint_surface_restricts_cors_to_the_platform_domain(plugin_configs):
    cors = plugin_configs["context-endpoint"]["cors"]
    assert "allow_origins" not in cors, "a wildcard origin would hand responses to any page"
    # The dots of the domain are a character class, not a bare `.`: see
    # tests/test_foreign_origin.py for what an unescaped one lets through (T-1679).
    assert cors["allow_origins_by_regex"] == [r"^https://[^@/]+\.joinedcontext[.]test$"]
    assert cors["allow_credential"] is False


def test_every_config_variable_is_declared_for_the_nginx_workers(rendered):
    """An undeclared ${{VAR}} costs a whole rule file, silently.

    APISIX resolves them with `os.getenv` inside an nginx worker, and a worker sees only the
    variables nginx.conf declares. An undeclared one makes APISIX reject the entire
    apisix.yaml and keep serving the last file that parsed, so route changes appear to be
    ignored while the ConfigMap looks perfectly correct.
    """
    docs = rendered("local")
    # A ConfigMap a Job fills at run time renders no data (postgres-cluster-dr-drill, T-0046).
    configmaps = {d["metadata"]["name"]: d.get("data") or {} for d in docs if d.get("kind") == "ConfigMap"}
    referenced = set(re.findall(r"\$\{\{(\w+)\}\}", configmaps["apisix-standalone-base"]["apisix.yaml"]))
    declared = set(yaml.safe_load(configmaps["apisix"]["config.yaml"])["nginx_config"]["envs"])
    assert referenced <= declared, f"not declared in apisix.nginx.envs: {sorted(referenced - declared)}"
    # The edge login needs both (ADR-N-019): the client secret and the session key.
    assert {"EDGE_CLIENT_SECRET", "OIDC_SESSION_SECRET"} <= referenced


def test_the_edge_client_secret_reaches_apisix_from_a_secret_only(rendered):
    """AP-27: the `edge` client's secret is an environment variable from the Secret the
    secrets component generates for the client, in the gateway's namespace; it appears
    nowhere in the rule file (the rule file carries the `${{VAR}}` reference)."""
    docs = rendered("local")
    deployment = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "apisix")
    env = {e["name"]: e for e in deployment["spec"]["template"]["spec"]["containers"][0]["env"]}
    ref = env["EDGE_CLIENT_SECRET"]["valueFrom"]["secretKeyRef"]
    assert ref == {"name": "keycloak-client-edge", "key": "client-secret"}
    namespace = deployment["metadata"]["namespace"]
    assert any(
        d.get("kind") == "Secret" and d["metadata"]["name"] == "keycloak-client-edge"
        and d["metadata"]["namespace"] == namespace
        for d in docs
    ), f"no keycloak-client-edge Secret rendered in {namespace}"
    # The plugin dials the PUBLIC issuer host, the same hairpin the Portal makes (T-0400): a
    # meshed pod needs 443 out of the outbound redirect, and the NetworkPolicy needs the port.
    annotations = deployment["spec"]["template"]["metadata"]["annotations"]
    assert "443" in annotations["config.linkerd.io/skip-outbound-ports"].split(",")
    policy = next(d for d in docs if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "apisix")
    # The hop is DNATed to the ingress controller's pod, which the rule names (T-6218).
    issuer = [
        {p["port"] for p in rule["ports"]}
        for rule in policy["spec"]["egress"]
        if any("traefik" in str(to.get("podSelector")) for to in rule.get("to", []))
    ]
    assert issuer and {443, 8443} <= issuer[0], issuer



def test_apisix_serves_the_composed_secret_and_helm_seeds_it_with_the_base(rendered):
    """ADR-N-030, AP-112: the rule file APISIX mounts is the Secret the Portal composes, never a
    ConfigMap, since it carries every App's client secret. Helm renders the base as a ConfigMap
    and seeds the Secret with the same file, kept across syncs once the Portal owns it."""
    docs = rendered("local")
    deployment = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "apisix")
    volumes = {v["name"]: v for v in deployment["spec"]["template"]["spec"]["volumes"]}
    assert volumes["apisix-admin"] == {
        "name": "apisix-admin",
        "secret": {"secretName": "apisix-standalone-config"},
    }
    mounts = {m["name"]: m for m in deployment["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]}
    assert mounts["apisix-admin"]["mountPath"] == "/apisix-config"
    assert "subPath" not in mounts["apisix-admin"], "a subPath mount never sees the reconciler's writes"

    namespace = deployment["metadata"]["namespace"]
    by_kind = {(d["kind"], d["metadata"]["name"]): d for d in docs if d["metadata"].get("namespace") == namespace}
    assert ("ConfigMap", "apisix-standalone-config") not in by_kind
    base = by_kind[("ConfigMap", "apisix-standalone-base")]["data"]["apisix.yaml"]
    seed = by_kind[("Secret", "apisix-standalone-config")]
    assert seed["stringData"]["apisix.yaml"] == base
    assert base.rstrip().endswith("#END")
    assert seed["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"


def test_the_portal_may_read_the_base_and_update_the_served_file_and_nothing_else(rendered):
    """ADR-N-030: the Portal's ServiceAccount gets read on the base ConfigMap and read/update on
    the one Secret in the APISIX namespace, each by name. ADR-N-037: it also makes and retires
    each App's Ingress and Certificate there; no update or patch on either, no other object."""
    docs = rendered("local")
    role = next(d for d in docs if d.get("kind") == "Role" and d["metadata"]["name"] == "edge-file-composer")
    assert role["rules"] == [
        {"apiGroups": [""], "resources": ["configmaps"], "resourceNames": ["apisix-standalone-base"], "verbs": ["get"]},
        {"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["apisix-standalone-config"], "verbs": ["get", "update"]},
        {"apiGroups": ["networking.k8s.io"], "resources": ["ingresses"], "verbs": ["get", "list", "create", "delete"]},
        {"apiGroups": ["cert-manager.io"], "resources": ["certificates"], "verbs": ["get", "list", "create", "delete"]},
    ]
    binding = next(d for d in docs if d.get("kind") == "RoleBinding" and d["metadata"]["name"] == "edge-file-composer")
    portal = next(
        d for d in docs
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"
    )
    assert binding["metadata"]["namespace"] == role["metadata"]["namespace"]
    assert binding["subjects"] == [{
        "kind": "ServiceAccount",
        "name": portal["spec"]["template"]["spec"]["serviceAccountName"],
        "namespace": portal["metadata"]["namespace"],
    }]


def edge_objects(docs: list[dict]) -> dict[str, dict]:
    kinds = ("Certificate", "TLSOption", "Ingress")
    # The staging Certificate and the MQTT brokers' own TLSOption (T-5562) are not the edge's.
    others = ("apisix-edge-staging", "mqtt-bsi-tr-02102")
    return {
        d["kind"]: d
        for d in docs
        if d.get("kind") in kinds and d["metadata"]["name"] not in others
    }


def staging_objects(docs: list[dict]) -> dict[str, dict]:
    return {
        d["kind"]: d
        for d in docs
        if d.get("kind") in ("Issuer", "Certificate", "ClusterIssuer")
        and d["metadata"]["name"] in ("letsencrypt-staging", "apisix-edge-staging")
    }


def test_new_hostnames_are_proven_on_the_staging_issuer_first(rendered, rendered_variant):
    """T-2806, OPS-35: a new domain meets Let's Encrypt STAGING before production carries it.

    HTTP-01 per hostname (no DNS API exists for the zone, so no DNS-01 and no wildcard), a
    namespaced Issuer so nothing cluster-wide changes, and a Secret nothing serves.
    """
    # dev proved dev.joinedcontext.com this way on 2026-09-24 before `domain` moved to it; a
    # next move sets stagingDomain the same way.
    docs = rendered_variant("dev", lambda tree: set_global(tree, "ingress.stagingDomain", "next.example.org"))
    objects = staging_objects(docs)
    assert "ClusterIssuer" not in objects
    issuer = objects["Issuer"]["spec"]["acme"]
    assert issuer["server"] == "https://acme-staging-v02.api.letsencrypt.org/directory"
    assert issuer["solvers"] == [{"http01": {"ingress": {"ingressClassName": "traefik"}}}]
    assert "email" not in issuer, "no personal address in Git; the ACME account needs none"

    certificate = objects["Certificate"]["spec"]
    assert certificate["issuerRef"] == {"name": "letsencrypt-staging", "kind": "Issuer"}
    assert certificate["secretName"] == "apisix-edge-staging-tls"
    assert certificate["dnsNames"] == [
        "next.example.org",
        "ai.next.example.org",
        "api.next.example.org",
        "assistant.next.example.org",
        "data.next.example.org",
        "idm.next.example.org",
        "opendata.next.example.org",
        "portal.next.example.org",
        "status.next.example.org",
        "tiles.next.example.org",
        "viz.next.example.org",
    ]
    assert not any("*" in name for name in certificate["dnsNames"]), "no wildcard over HTTP-01"

    served = edge_objects(docs)["Ingress"]["spec"]["tls"][0]["secretName"]
    assert served != certificate["secretName"], "a staging certificate is never served"


def test_no_staging_objects_without_a_staging_domain(rendered):
    """Production, and dev once its domain moved, render no staging Issuer or Certificate."""
    assert staging_objects(rendered("production")) == {}
    assert staging_objects(rendered("dev")) == {}


def test_certificate_covers_the_apex_and_every_routed_subdomain(rendered):
    """T-0045: the SAN list is explicit in Git, not inferred by cert-manager's ingress-shim."""
    objects = edge_objects(rendered("dev"))
    certificate = objects["Certificate"]
    assert certificate["spec"]["secretName"] == "apisix-edge-tls"
    assert certificate["spec"]["issuerRef"] == {"name": "letsencrypt-prod", "kind": "ClusterIssuer"}
    # One entry per hostname: the Portal's three routes share `portal` and must not put it
    # on the certificate three times (ADR-N-019).
    assert certificate["spec"]["dnsNames"] == [
        "dev.joinedcontext.com",
        "ai.dev.joinedcontext.com",
        "api.dev.joinedcontext.com",
        "assistant.dev.joinedcontext.com",
        "data.dev.joinedcontext.com",
        "idm.dev.joinedcontext.com",
        "opendata.dev.joinedcontext.com",
        "portal.dev.joinedcontext.com",
        "status.dev.joinedcontext.com",
        "tiles.dev.joinedcontext.com",
        "viz.dev.joinedcontext.com",
    ]

    ingress = objects["Ingress"]
    hosts = [rule["host"] for rule in ingress["spec"]["rules"]]
    assert hosts == certificate["spec"]["dnsNames"], "the apex must be routed, not only certified"
    assert ingress["spec"]["tls"][0]["hosts"] == certificate["spec"]["dnsNames"]
    assert ingress["spec"]["tls"][0]["secretName"] == certificate["spec"]["secretName"]
    # Two owners of one Secret means two ACME orders for the same names.
    assert "cert-manager.io/cluster-issuer" not in ingress["metadata"]["annotations"]


def test_traefik_listener_enforces_tr_02102(rendered):
    """T-0044, OPS-36: TLS 1.2 floor, AEAD suites only (BSI TR-02102)."""
    objects = edge_objects(rendered("dev"))
    options = objects["TLSOption"]["spec"]
    assert options["minVersion"] == "VersionTLS12"
    assert options["maxVersion"] == "VersionTLS13"
    for suite in options["cipherSuites"]:
        assert suite.startswith("TLS_ECDHE_"), suite
        assert all(marker not in suite for marker in FORBIDDEN_CIPHER_MARKERS), suite

    annotations = objects["Ingress"]["metadata"]["annotations"]
    assert annotations["traefik.ingress.kubernetes.io/router.tls.options"] == "dev-bsi-tr-02102@kubernetescrd"


def websocket_edge(docs: list[dict]) -> dict[str, dict]:
    return {
        d["kind"]: d
        for d in docs
        if d.get("kind") in ("Middleware", "IngressRoute")
        and d["metadata"]["name"] in ("endpoint-ws-upgrade", "endpoint-ws")
    }


def test_traefik_names_the_upgrade_on_endpoint_websocket_answers(rendered):
    """T-5931, RFC 9110 15.5.22: the 426 of a plain GET on `/api/endpoint/{slug}/ws` names the
    upgrade. Upgrade is hop-by-hop, so Traefik drops the gateway's; the edge sets it again on these
    paths only, behind the same certificate, TLS policy and APISIX Service as the edge Ingress."""
    docs = rendered("dev")
    objects = edge_objects(docs)
    ws = websocket_edge(docs)
    headers = ws["Middleware"]["spec"]["headers"]["customResponseHeaders"]
    assert headers == {"Upgrade": "websocket", "Connection": "Upgrade"}
    route = ws["IngressRoute"]["spec"]
    [rule] = route["routes"]
    assert rule["middlewares"] == [{"name": "endpoint-ws-upgrade"}]
    assert rule["match"].endswith(" && PathRegexp(`^/api/endpoint/[^/]+/ws$`)"), rule["match"]
    hosts = set(re.findall(r"Host\(`([^`]+)`\)", rule["match"]))
    domain = objects["Certificate"]["spec"]["dnsNames"][0]
    # Every host an Endpoint WebSocket route of APISIX answers on, and only hosts the certificate covers.
    assert {domain, f"api.{domain}", f"portal.{domain}"} <= hosts <= set(objects["Certificate"]["spec"]["dnsNames"])
    ingress = objects["Ingress"]
    backend = ingress["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]
    assert rule["services"] == [{"name": backend["name"], "port": backend["port"]["number"]}]
    assert route["tls"]["secretName"] == ingress["spec"]["tls"][0]["secretName"]
    assert route["tls"]["options"]["name"] == objects["TLSOption"]["metadata"]["name"]
    assert route["entryPoints"] == ["websecure"]


def test_no_traefik_websocket_route_on_another_ingress_controller(rendered):
    assert websocket_edge(rendered("production")) == {}


def test_nginx_listener_carries_the_same_cipher_policy(rendered):
    """OPS-36: the nginx branch of the same requirement; no TLSOption CRD exists there."""
    objects = edge_objects(rendered("production"))
    assert "TLSOption" not in objects
    annotations = objects["Ingress"]["metadata"]["annotations"]
    ciphers = annotations["nginx.ingress.kubernetes.io/ssl-ciphers"].split(":")
    assert ciphers, "no cipher list configured"
    for cipher in ciphers:
        assert cipher.startswith("ECDHE-"), cipher
        assert "CBC" not in cipher and "3DES" not in cipher, cipher
    assert annotations["nginx.ingress.kubernetes.io/ssl-prefer-server-ciphers"] == "true"


def test_the_proxy_server_names_its_own_body_ceiling(rendered):
    """T-2263: with the inherited `client_max_body_size 0` the edge answers 413 to every
    chunked request body.

    For a body with a `Content-Length` nginx reads `0` as "no limit", but its chunked filter
    compares `content_length_n + chunk` against the same number without a zero guard, so a
    streamed body of any size is over the limit — measured on dev as `client intended to send
    too large chunked body: 0+435 bytes` on a Portal write. The ceiling therefore has to be a
    number, and it has to be set in the proxy server block: APISIX's own
    `client_max_body_size 0` already sits in the http block, and nginx refuses to start on a
    duplicate there.
    """
    docs = rendered("local")
    nginx = yaml.safe_load(
        next(
            d for d in docs
            if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix"
        )["data"]["config.yaml"]
    )["nginx_config"]
    server = nginx.get("http_server_configuration_snippet", "")
    found = re.findall(r"^\s*client_max_body_size\s+(\S+?);", server, re.MULTILINE)
    assert found, f"the proxy server block sets no client_max_body_size: {server!r}"
    assert found[0] != "0", "0 makes nginx refuse every chunked body"
    assert re.fullmatch(r"\d+[km]?", found[0], re.IGNORECASE), found[0]
    # A service is the authority over its own bodies: the largest one a service accepts is
    # the 11 MB of the Portal's pipeline test, so the edge must not cut below that.
    assert found[0].lower().endswith("m") and int(found[0][:-1]) >= 12, (
        f"the edge ceiling {found[0]} is below what a service already accepts"
    )
    assert "client_max_body_size" not in nginx.get("http_configuration_snippet", ""), (
        "a second client_max_body_size in the http block makes nginx refuse to start"
    )


def wildcard_objects(docs: list[dict]) -> dict[tuple[str, str], dict]:
    names = {"letsencrypt-dns01", "apisix-apps-wildcard", "cert-manager-webhook-hetzner",
             "cert-manager-webhook-hetzner:read-secrets"}
    return {(d["kind"], d["metadata"]["name"]): d for d in docs
            if d.get("kind") and d.get("metadata", {}).get("name") in names}


def test_the_app_wildcard_is_one_dns01_certificate_on_the_edge(rendered_variant):
    """ADR-N-037 §6, AP-133 (T-3013): with appsWildcard on, one certificate `*.apps.<domain>` by
    DNS-01 in the delegated App zone serves every App host from the edge Ingress, and the solver
    reads its token from a mounted file with no right to read any Secret."""
    docs = rendered_variant("dev", lambda tree: set_global(
        tree, "ingress.appsWildcard", {"enabled": True, "certManagerNamespace": "cert-manager", "tokenSecret": "hetzner-dns"}))
    objects = wildcard_objects(docs)

    issuer = objects[("Issuer", "letsencrypt-dns01")]["spec"]["acme"]
    assert issuer["server"] == "https://acme-v02.api.letsencrypt.org/directory"
    assert "email" not in issuer
    [solver] = issuer["solvers"]
    assert solver["selector"] == {"dnsZones": ["apps.dev.joinedcontext.com"]}
    assert solver["dns01"]["webhook"] == {
        "groupName": "acme.hetzner.com", "solverName": "hetzner",
        "config": {"tokenFilePath": "/var/run/secrets/hetzner/token"},
    }, "the token is a file, never tokenSecretKeyRef (which needs the read-every-Secret grant)"

    certificate = objects[("Certificate", "apisix-apps-wildcard")]
    assert certificate["metadata"]["namespace"] == objects[("Issuer", "letsencrypt-dns01")]["metadata"]["namespace"]
    assert certificate["spec"] == {
        "secretName": "apisix-apps-wildcard-tls",
        "issuerRef": {"name": "letsencrypt-dns01", "kind": "Issuer"},
        "dnsNames": ["*.apps.dev.joinedcontext.com"],
    }

    ingress = edge_objects(docs)["Ingress"]["spec"]
    assert {"hosts": ["*.apps.dev.joinedcontext.com"], "secretName": "apisix-apps-wildcard-tls"} in ingress["tls"]
    assert "*.apps.dev.joinedcontext.com" in [rule["host"] for rule in ingress["rules"]]
    # The Portal copies the first rule as every App Ingress's template (AP-133): still the apex.
    assert ingress["rules"][0]["host"] == "dev.joinedcontext.com"
    # The edge certificate itself never carries the wildcard: HTTP-01 cannot prove one.
    assert not any("*" in name for name in edge_objects(docs)["Certificate"]["spec"]["dnsNames"])

    assert objects[("ClusterRole", "cert-manager-webhook-hetzner:read-secrets")]["rules"] == []
    webhook = objects[("Deployment", "cert-manager-webhook-hetzner")]
    assert webhook["metadata"]["namespace"] == "cert-manager"
    pod = webhook["spec"]["template"]["spec"]
    [container] = pod["containers"]
    assert container["image"] == (
        "docker.io/hetzner/cert-manager-webhook-hetzner:v0.9.0"
        "@sha256:b64db89ba4c1f008aeaf3d834681d4227316c78e0c3ab6c21939457e90d42586")
    assert {"name": "hetzner-token", "mountPath": "/var/run/secrets/hetzner", "readOnly": True} in container["volumeMounts"]
    assert {"name": "hetzner-token", "secret": {"secretName": "hetzner-dns", "items": [{"key": "token", "path": "token"}]}} in pod["volumes"]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["resources"]["limits"]["memory"]


def test_without_the_app_wildcard_nothing_of_it_renders(rendered):
    """Off by default: every App host keeps its own HTTP-01 certificate (ADR-N-037 §3.2)."""
    for env in ("dev", "production"):
        docs = rendered(env)
        assert wildcard_objects(docs) == {}, env
        ingress = edge_objects(docs)["Ingress"]["spec"]
        assert not any("*" in rule["host"] for rule in ingress["rules"]), env


def test_every_answer_gets_the_header_floor_where_nothing_set_it(rendered):
    """T-5107 (ZAP 10035, 10063): a global rule adds HSTS, nosniff and a Permissions-Policy to an
    answer that has none, the edge's own 404 for an address no route serves included; it never
    replaces what the upstream or the route set, and leaves an App's host its own policy."""
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    rules = {r["id"]: r["plugins"] for r in yaml.safe_load(cm["data"]["apisix.yaml"])["global_rules"]}
    post = rules["edge-headers"]["serverless-post-function"]
    assert post["phase"] == "header_filter"
    lua = "".join(post["functions"])
    for header, value in (
        ("Strict-Transport-Security", "max-age=31536000; includeSubDomains; preload"),
        ("X-Content-Type-Options", "nosniff"),
        ("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    ):
        assert f'h["{header}"] == nil' in lua and f'h["{header}"] = "{value}"' in lua, header
    assert re.search(r'local apps = "\.apps\.[a-z0-9.-]+"', lua), "the App hosts keep their own policy"


@pytest.mark.parametrize("env", ["local", "dev"])
def test_an_apps_ingress_may_carry_what_the_edge_ingress_carries(rendered, env):
    """F-R2DEP2-006: the Portal copies the edge Ingress's class and annotations into each App's
    Ingress (app_hosts.rs), and the portal-edge admission policy admits those alone. An
    annotation added to the edge Ingress and not to that list would stop every App publishing."""
    docs = rendered(env)
    policy = next(d for d in docs if d.get("kind") == "ValidatingAdmissionPolicy"
                  and d["metadata"]["name"].endswith("-portal-edge"))
    checks = " ".join(v["expression"] for v in policy["spec"]["validations"])
    edge = [d for d in docs if d.get("kind") == "Ingress" and d["metadata"]["name"].startswith("apisix")]
    assert edge, "no edge Ingress rendered"
    for ingress in edge:
        assert f'object.spec.ingressClassName == "{ingress["spec"]["ingressClassName"]}"' in checks
        for key in ingress["metadata"].get("annotations") or {}:
            assert f'"{key}"' in checks, f"{key} of {ingress['metadata']['name']} is not admitted on an App's Ingress"
