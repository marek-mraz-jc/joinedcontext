"""jc-prod's edge in host mode reaches jc only on the hosts it names (T-6705).

jc serves some modules inside the pod only: shako (`/git`), ekiben (`/streams`, or its default host
`streams.{domain}`) and the in-binary broker (`/broker`, or `broker.{domain}`); the Portal and the
gateway call them on jc's own listener under their host. A route with no host answers every Host,
so a pod that reaches the edge could name one of those hosts and reach the module past the
gateway. Every jc route names its host, and the uri-blocker keeps the paths closed on the hosts
it does name.
"""

import http.client
import json
import re
import shutil
import subprocess
import time
import tomllib

import pytest
import yaml

from test_jc_app_hosts import edge_file

ENV = "jc-prod"
DOMAIN = "smartcity.joinedcontext.com"
JC = "jc.jc-prod.svc.cluster.local:8080"
BLOCKED = re.compile(r"^/(git|streams|broker|metrics)([/?]|$)")


@pytest.fixture(scope="module")
def edge(rendered):
    return edge_file(rendered(ENV))


def jc_routes(edge):
    upstreams = {u["id"]: u for u in edge["upstreams"]}
    return [r for r in edge["routes"] if JC in upstreams[r["upstream_id"]]["nodes"]]


def test_every_jc_route_names_its_host(edge):
    routes = jc_routes(edge)
    assert {r["id"] for r in routes} >= {"jc-host-apex", "jc-host-api", "jc"}
    for route in routes:
        assert route.get("host") or route.get("hosts"), f"{route['id']} answers every Host"
    hosts = {r.get("host") for r in routes} | {h for r in routes for h in r.get("hosts", [])}
    assert DOMAIN in hosts
    for inside in (f"broker.{DOMAIN}", f"streams.{DOMAIN}", "localhost:8080", "localhost"):
        assert inside not in hosts, inside


# Requests that must reach no jc module: a host jc serves inside the pod only, or a blocked path
# in any spelling on a host the edge does route.
FORGED = [
    ("localhost:8080", "/git/api/v1/version"),
    ("localhost:8080", "/streams/ping"),
    ("localhost:8080", "/broker/ngsi-ld/v1/entities"),
    (f"broker.{DOMAIN}", "/"),
    (f"broker.{DOMAIN}", "/ngsi-ld/v1/entities"),
    (f"streams.{DOMAIN}", "/"),
    (f"streams.{DOMAIN}", "/ping"),
    (DOMAIN, "/git/api/v1/version"),
    (DOMAIN, "/%67it/api/v1/version"),
    (DOMAIN, "//git/api/v1/version"),
    (DOMAIN, "/./git/api/v1/version"),
    (DOMAIN, "/streams/ping"),
    (DOMAIN, "/broker/ngsi-ld/v1/entities"),
    (f"api.{DOMAIN}", "/git/api/v1/version"),
    (f"portal.{DOMAIN}", "//broker/ngsi-ld/v1/entities"),
    # jc's public listener answers /metrics too (G-17, T-6727): the series stay on the admin port.
    (f"api.{DOMAIN}", "/metrics"),
    (f"api.{DOMAIN}", "/%6detrics"),
    (f"api.{DOMAIN}", "//metrics?x=1"),
    (DOMAIN, "/metrics"),
    (f"data.{DOMAIN}", "/metrics"),
]
# And the hosts it routes still reach jc.
ROUTED = [(DOMAIN, "/grafana/"), (f"portal.{DOMAIN}", "/"), (f"api.{DOMAIN}", "/api/endpoint/x")]


def ask(port: int, host: str, path: str) -> tuple[int, str]:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.putrequest("GET", path, skip_host=True, skip_accept_encoding=True)
        conn.putheader("Host", host)
        conn.endheaders()
        response = conn.getresponse()
        return response.status, response.read().decode(errors="replace")
    finally:
        conn.close()


@pytest.mark.xdist_group("docker-apisix-jc-host")
@pytest.mark.skipif(not shutil.which("docker"), reason="docker not installed")
def test_the_pinned_edge_reaches_no_inside_module_on_a_forged_host_or_path(edge, tmp_path):
    """Played on `apache/apisix` as pinned with jc-prod's rendered jc routes and their plugin
    configs; jc is an echo route that names what it was sent."""
    from test_apisix_429_contract import pinned_image
    from test_edge_csp_baseline import free_port

    routes = jc_routes(edge)
    ids = {r["upstream_id"] for r in routes}
    configs = {r.get("plugin_config_id") for r in routes}
    echo = (
        'return function() ngx.say(require("cjson").encode({reached = true, '
        'host = ngx.var.http_x_sent_host, uri = ngx.var.request_uri})); ngx.exit(200) end'
    )
    upstreams = [
        {"id": u, "type": "roundrobin", "nodes": {"127.0.0.1:9080": 1},
         "pass_host": "rewrite", "upstream_host": "echo.test"}
        for u in sorted(ids)
    ]
    played = [dict(r) for r in routes]
    played.append({"id": "echo", "uri": "/*", "host": "echo.test", "priority": 100,
                   "plugins": {"serverless-pre-function": {"phase": "rewrite", "functions": [echo]}}})
    plugin_configs = []
    for config in edge.get("plugin_configs", []):
        if config["id"] in configs:
            config = json.loads(json.dumps(config))
            # The echo learns which Host the edge forwarded.
            rewrite = config["plugins"].setdefault("proxy-rewrite", {})
            rewrite.setdefault("headers", {}).setdefault("set", {})["X-Sent-Host"] = "$host"
            plugin_configs.append(config)
    (tmp_path / "config.yaml").write_text(
        "deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n"
    )
    (tmp_path / "apisix.yaml").write_text(
        yaml.safe_dump({"routes": played, "upstreams": upstreams, "plugin_configs": plugin_configs})
        + "#END\n"
    )
    port = free_port()
    name = f"apisix-jc-host-{port}"
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    started = subprocess.run(
        ["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:9080",
         "-v", f"{tmp_path / 'config.yaml'}:/usr/local/apisix/conf/config.yaml:ro",
         "-v", f"{tmp_path / 'apisix.yaml'}:/usr/local/apisix/conf/apisix.yaml:ro",
         pinned_image()],
        capture_output=True, text=True, check=False,
    )
    if started.returncode != 0:
        pytest.skip(f"the pinned image does not run here: {started.stderr.strip()}")
    try:
        deadline = time.time() + 60
        while True:
            try:
                status, body = ask(port, *ROUTED[0])
                if status == 200:
                    break
            except OSError:
                pass
            if time.time() > deadline:
                logs = subprocess.run(["docker", "logs", name], capture_output=True, text=True).stderr
                pytest.fail(f"APISIX did not start or route {ROUTED[0]}:\n{logs[-3000:]}")
            time.sleep(1)
        answers = {(h, p): ask(port, h, p) for h, p in FORGED + ROUTED}
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    for host, path in ROUTED:
        status, body = answers[(host, path)]
        assert status == 200 and json.loads(body)["host"] == host, (host, path, status, body)
    for host, path in FORGED:
        status, body = answers[(host, path)]
        if status != 200 or '"reached":true' not in body:
            continue
        # Forwarded: jc routes the raw path by prefix (no decoding, no dot segments), so it
        # must name no blocked mount, on a host jc's edge routes.
        seen = json.loads(body)
        assert seen["host"] in {DOMAIN, f"api.{DOMAIN}", f"portal.{DOMAIN}"}, (host, path, seen)
        assert not BLOCKED.match(seen["uri"]), (host, path, seen)


def test_the_tiles_host_is_this_installations_own_basemap(edge, rendered):
    # The dataset map stayed an empty grey box (owner 2026-11-11): jc named no tiles address, and
    # dev's kept basemap-tiles wrote tiles.dev into its style and admitted only viz.dev.
    upstreams = {u["id"]: u for u in edge["upstreams"]}
    tiles = [r for r in edge["routes"] if r.get("host") == f"tiles.{DOMAIN}"]
    assert [list(upstreams[r["upstream_id"]]["nodes"]) for r in tiles] == [["basemap-tiles.jc-prod.svc.cluster.local:8080"]]
    docs = rendered(ENV)
    (cm,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "jc"
             and d["metadata"]["namespace"] == "jc-prod"]
    assert tomllib.loads(cm["data"]["jc.toml"])["dataroom"]["addresses"] == {"tiles": f"https://tiles.{DOMAIN}"}
    (styles,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "basemap-tiles-styles"]
    assert json.loads(styles["data"]["light-sk.json"])["sources"]["protomaps"]["url"] == f"pmtiles://https://tiles.{DOMAIN}/basemap.pmtiles"
    assert not [d for d in docs if d.get("kind") == "NetworkPolicy" and d["metadata"]["namespace"] == "dev"
                and "basemap" in d["metadata"]["name"]]


def test_the_api_host_answers_the_platforms_own_origins_like_old_prod(edge):
    """The catalogue's viz iframe and Apps read api.{domain} across origins; old prod's
    context-endpoint and named-MCP `cors` rules carry over, never `*` or a look-alike domain."""
    routes = {r["id"]: r for r in jc_routes(edge)}
    rule = re.compile(routes["jc-host-api-endpoint"]["plugins"]["cors"]["allow_origins_by_regex"][0])
    for route_id, uri, methods in (("jc-host-api-endpoint", "/api/endpoint/*", "GET,HEAD,OPTIONS"),
                                   ("jc-host-api-mcp", "/api/mcp*", "POST,OPTIONS")):
        route = routes[route_id]
        cors = route["plugins"]["cors"]
        assert (route["host"], route["uri"], cors["allow_methods"]) == (f"api.{DOMAIN}", uri, methods)
        assert cors["allow_credential"] is False and "allow_origins" not in cors
        assert route["priority"] > routes["jc-host-api"]["priority"]
        assert route["plugin_config_id"] == routes["jc-host-api"]["plugin_config_id"]  # headers cleared, uri-blocker
    # The grid's total and the map's unplaced count read NGSILD-Results-Count across origins (owner
    # 2026-11-11 10:05Z: no total on jc-prod's catalogue), as old prod's endpoint rule exposed it.
    assert routes["jc-host-api-endpoint"]["plugins"]["cors"]["expose_headers"] == "NGSILD-Results-Count,Content-Range,Link"
    for origin in (f"https://viz.{DOMAIN}", f"https://app-praha-mapa.apps.{DOMAIN}", f"https://portal.{DOMAIN}"):
        assert rule.match(origin), origin
    for origin in ("https://evil.smartcity-joinedcontext.com", f"http://viz.{DOMAIN}", f"https://{DOMAIN}.evil.org",
                   f"https://a@viz.{DOMAIN}", "https://example.org", "null"):
        assert not rule.match(origin), origin
    # The Portal's own routes stay without `cors` (test_foreign_origin's rule for jc hosts).
    for route_id in ("jc", "jc-host-apex", "jc-host-api"):
        assert "cors" not in (routes[route_id].get("plugins") or {}), route_id


def test_the_pods_token_requests_count_per_client_and_the_public_route_keeps_its_bucket(edge):
    """jc's back channel reaches idm from the pod network; a pipeline's token storm used the one
    pod-address bucket and starved every sign-in (T-6676 15:38Z). Pod-network token requests get a
    bucket per client, still limited; every other source keeps jc-identity's per-address bucket."""
    routes = {r["id"]: r for r in edge["routes"]}
    pod, public = routes["jc-identity-pod-token"], routes["jc-identity"]
    assert (pod["host"], pod["uri"]) == (f"idm.{DOMAIN}", "/realms/*/protocol/openid-connect/token")
    assert pod["vars"] == [["remote_addr", "ipmatch", ["10.42.0.0/16"]]]
    assert pod["priority"] > public["priority"]
    limit = pod["plugins"]["limit-count"]
    assert (limit["count"], limit["time_window"], limit["key_type"], limit["key"]) == (
        600, 60, "var_combination", "$post_arg_client_id $remote_user")
    assert pod["plugin_config_id"] == public["plugin_config_id"]  # headers cleared, uri-blocker
    # The public route is untouched: no vars, no route-level limit over the jc config's.
    assert not public.get("vars") and "limit-count" not in (public.get("plugins") or {})
    jc_config = next(c for c in edge["plugin_configs"] if c["id"] == public["plugin_config_id"])
    assert jc_config["plugins"]["limit-count"]["key"] == "remote_addr"


def test_the_pods_endpoint_writes_count_per_bearer_like_old_prod(edge):
    """jc's pipelines write api.{domain} from the pod network, all from one pod address; old prod
    counted the endpoint surface per bearer and address at 1200/60 s. Only the pod network gets
    that route; public callers keep the per-address bucket and the cors route."""
    routes = {r["id"]: r for r in edge["routes"]}
    pod, public = routes["jc-host-api-endpoint-pod"], routes["jc-host-api-endpoint"]
    assert (pod["host"], pod["uri"]) == (f"api.{DOMAIN}", "/api/endpoint/*")
    assert pod["vars"] == [["remote_addr", "ipmatch", ["10.42.0.0/16"]]]
    assert pod["priority"] > public["priority"] > routes["jc-host-api"]["priority"]
    limit = pod["plugins"]["limit-count"]
    assert (limit["count"], limit["time_window"], limit["key_type"], limit["key"]) == (
        1200, 60, "var_combination", "$http_authorization $remote_addr")
    assert pod["plugin_config_id"] == public["plugin_config_id"]  # headers cleared, uri-blocker
    assert not public.get("vars") and "limit-count" not in public["plugins"]
