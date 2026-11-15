"""T-5900: the edge keeps the gateway's anonymous reads of data a few seconds, and nothing private.

The first tests read the committed route and the gateway's values. The last runs the pinned APISIX
image with the committed `proxy-cache` block in front of a stub that answers as the gateway does
(`public, max-age=5` to an anonymous read, `private, no-store` to everything else), because only
answers prove what the edge stores: a token's view, a session and an answer the gateway keeps
private must never come back to someone else.
"""

import http.server
import itertools
import json
import re
import shutil
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

import pytest
import yaml

from test_apisix_429_contract import pinned_image
from test_apisix_routes import requires_helmfile

pytestmark = pytest.mark.xdist_group("docker-apisix-cache")

ROOT = Path(__file__).resolve().parent.parent
ROUTES = (ROOT / "components/context-gateway/apisix-routes.yaml").read_text()
ALL = yaml.safe_load(ROUTES.replace("{{", "#").replace("}}", ""))
ROUTE, HISTORY, BULK = ALL["api-context-endpoint"], ALL["api-context-endpoint-history"], ALL["api-context-endpoint-bulk"]
MCP, SCHEMA = ALL["api-context-endpoint-mcp"], ALL["api-context-endpoint-schema"]
VALUES = ROOT / "components/context-gateway/values/gateway/base-values.yaml.gotmpl"
PLUGINS = yaml.safe_load((ROOT / "components/context-gateway/apisix-plugins.yaml").read_text()
                         .replace("{{", "#").replace("}}", ""))


def test_only_the_anonymous_data_host_route_caches():
    assert (ROUTE["subDomain"], ROUTE["uri"], ROUTE["pluginConfig"]) == ("api", "/api/endpoint/*", "context-endpoint")
    cache = ROUTE["plugins"]["proxy-cache"]
    assert (cache["cache_strategy"], cache["cache_zone"]) == ("disk", "disk_cache_one")
    # The gateway's max-age decides; a `private` or `no-store` answer is never stored.
    assert cache["cache_control"] is True
    for field in ("cache_bypass", "no_cache"):
        assert set(cache[field]) == {"$http_cookie", "$http_authorization", "$http_link", "$http_prefer",
                                     "$http_ngsild_tenant"}, field
    assert cache["cache_http_status"] == [200] and cache["cache_method"] == ["GET", "HEAD"]
    # Every request header the gateway's Vary names, Authorization aside (it bypasses).
    for var in ("$host", "$request_uri", "$http_accept", "$http_accept_language", "$http_accept_encoding",
                "$http_ngsild_results_restricted"):
        assert var in cache["cache_key"], var
    # The data host's plain reads and its history (T-6185): the apex and Portal-host routes carry
    # bearers and never cache, and the bulk route's `file.*` exports are never stored.
    assert ROUTES.count("proxy-cache:") == 2
    assert "proxy-cache" not in BULK["plugins"]


def test_the_history_route_caches_as_the_reads_do_and_keeps_the_bulk_limit():
    """T-6185: the temporal history leaves the bulk route for one of its own on the data host, with
    the reads' cache block and the bulk route's concurrency limit; the exports stay where they were."""
    assert (HISTORY["subDomain"], HISTORY["uri"], HISTORY["pluginConfig"]) == ("api", "/api/endpoint/*", "context-endpoint")
    assert HISTORY["plugins"]["proxy-cache"] == ROUTE["plugins"]["proxy-cache"]
    assert HISTORY["plugins"]["limit-conn"] == BULK["plugins"]["limit-conn"]
    assert HISTORY["timeout"] == BULK["timeout"] and HISTORY["priority"] > BULK["priority"]
    history = re.compile(HISTORY["vars"][0][2])
    assert history.search("/api/endpoint/abc/ngsi-ld/v1/temporal/entities")
    assert history.search("/api/endpoint/abc/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:T:1")
    for path in ("/api/endpoint/abc/file.csv", "/api/endpoint/abc/ngsi-ld/v1/entities",
                 "/api/endpoint/file.csv/mcp", "/api/endpoint/abc/ws"):
        assert not history.search(path), path


def test_every_caching_route_buffers_after_its_plugin_config_merges():
    """APISIX sends an unbuffered request through its @disable_proxy_buffering location, where
    proxy_cache stores nothing: the chain's `disable_proxy_buffering: true` killed both data-host
    caches on dev (T-6185, 2026-10-31). A route plugin replaces the chain's plugin of that name."""
    caching = {name: r for name, r in ALL.items() if "proxy-cache" in r.get("plugins", {})}
    assert set(caching) == {"api-context-endpoint", "api-context-endpoint-history"}
    for name, route in caching.items():
        merged = {**PLUGINS[route["pluginConfig"]]["plugins"], **route["plugins"]}
        assert merged["proxy-buffering"] == {"disable_proxy_buffering": False}, name
    # The exports and the MCP streams keep streaming; a schema read keeps its If-None-Match (T-6507).
    for route in (BULK, MCP, SCHEMA):
        merged = {**PLUGINS[route["pluginConfig"]]["plugins"], **route.get("plugins", {})}
        assert merged["proxy-buffering"]["disable_proxy_buffering"] is True


def test_the_mcp_path_leaves_the_buffered_reads_route():
    """T-6185 review: the reads route buffers for its cache, which would hold an MCP SSE answer
    until it ends; `{endpoint}/mcp` keeps the plain chain on a route of its own above it."""
    assert (MCP["subDomain"], MCP["uri"], MCP["pluginConfig"]) == ("api", ROUTE["uri"], ROUTE["pluginConfig"])
    assert MCP["priority"] > ROUTE["priority"] and "plugins" not in MCP
    assert MCP["timeout"] == ROUTE["timeout"] and MCP["keepalivePool"] == ROUTE["keepalivePool"]
    mcp = re.compile(MCP["vars"][0][2])
    assert mcp.search("/api/endpoint/abc/mcp")
    for path in ("/api/endpoint/abc/mcp/x", "/api/endpoint/abc/ngsi-ld/v1/entities?q=mcp",
                 "/api/endpoint/abc/ngsi-ld/v1/temporal/entities", "/api/endpoint/mcp"):
        assert not mcp.search(path), path


def test_the_schema_path_leaves_the_caching_reads_route():
    """T-6507: `{endpoint}/schema/*` keeps the plain chain on a route of its own above the reads
    route, the reads route's limits and timeouts with it, and only that path moves."""
    assert (SCHEMA["subDomain"], SCHEMA["uri"], SCHEMA["pluginConfig"]) == ("api", ROUTE["uri"], ROUTE["pluginConfig"])
    assert SCHEMA["priority"] > ROUTE["priority"] and "plugins" not in SCHEMA
    assert SCHEMA["timeout"] == ROUTE["timeout"] and SCHEMA["keepalivePool"] == ROUTE["keepalivePool"]
    schema = re.compile(SCHEMA["vars"][0][2])
    for path in ("/api/endpoint/abc/schema/v1/linkml", "/api/endpoint/abc/schema/index.json"):
        assert schema.search(path), path
    for path in ("/api/endpoint/abc/ngsi-ld/v1/entities", "/api/endpoint/schema/mcp", "/api/endpoint/schema",
                 "/api/endpoint/abc/ngsi-ld/v1/temporal/entities", "/api/endpoint/abc/file.csv"):
        assert not schema.search(path), path


@requires_helmfile
def test_every_cache_zone_a_route_names_is_one_the_apisix_config_declares(rendered):
    """APISIX drops a route whose proxy-cache zone its config.yaml lacks, and the data host then
    answered every read with the edge's 404 (dev, 2026-10-28): the chart's config decides."""
    docs = rendered("dev")
    config = next(d for d in docs if d.get("kind") == "ConfigMap" and "config.yaml" in d.get("data", {})
                  and "proxy_cache" in d["data"]["config.yaml"])
    zones = {z["name"] for z in yaml.safe_load(config["data"]["config.yaml"])["apisix"]["proxy_cache"]["zones"]}
    standalone = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base")
    named = {r["plugins"]["proxy-cache"]["cache_zone"] for r in yaml.safe_load(standalone["data"]["apisix.yaml"])["routes"]
             if "proxy-cache" in r.get("plugins", {})}
    assert named and named <= zones, (named, zones)


def test_the_gateway_marks_anonymous_reads_public_for_a_minute():
    # T-6185: the gateway's upper bound (PUBLIC_MAX_AGE_LIMIT), so one hour's views of a chart
    # share its history reads; a revoked public grant reaches anonymous readers 60 s late at most.
    assert 'JC_GATEWAY_PUBLIC_MAX_AGE: "60"' in VALUES.read_text()


STREAM_PAUSE = 4


ARTIFACT_ETAG = '"artifact-1"'


class Gateway(http.server.BaseHTTPRequestHandler):
    counter = itertools.count()

    def do_GET(self):
        if "/schema/" in self.path:
            # A schema artifact as handlers/schema_routes.rs serves it (EP-51): a 304 only when the
            # reader's If-None-Match reaches this upstream.
            matched = self.headers.get("If-None-Match") == ARTIFACT_ETAG
            self.send_response(304 if matched else 200)
            self.send_header("ETag", ARTIFACT_ETAG)
            self.send_header("Cache-Control", "private, no-cache")
            self.end_headers()
            if not matched:
                self.wfile.write(b"id: model\n")
            return
        n = next(self.counter)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        anonymous = all(self.headers.get(h) is None for h in ("Authorization", "Cookie", "Link", "Prefer"))
        self.send_header("Cache-Control", "public, max-age=5" if anonymous else "private, no-store")
        self.send_header("Vary", "Authorization, Accept, Accept-Language, NGSILD-Results-Restricted, Accept-Encoding")
        self.end_headers()
        self.wfile.write(json.dumps({"n": n, "auth": self.headers.get("Authorization"), "link": self.headers.get("Link"),
                                     "lang": self.headers.get("Accept-Language")}).encode())

    def do_POST(self):
        # An MCP answer as the gateway streams it: one event now, the next after a pause.
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        self.wfile.write(b"event: message\ndata: {\"first\": true}\n\n")
        self.wfile.flush()
        time.sleep(STREAM_PAUSE)
        self.wfile.write(b"event: message\ndata: {\"last\": true}\n\n")
        self.wfile.flush()

    def log_message(self, *args):
        pass


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


@pytest.fixture(scope="module")
def edge(tmp_path_factory):
    if not shutil.which("docker"):
        pytest.skip("docker not installed")
    server = http.server.ThreadingHTTPServer(("0.0.0.0", 0), Gateway)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    tmp = tmp_path_factory.mktemp("api-cache")
    # The chart's zone list (apisix 2.15 config.yaml): the image's default would also hold
    # `memory_cache`, which the deployed config lacks.
    (tmp / "config.yaml").write_text("deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n"
                                     "apisix:\n  proxy_cache:\n    cache_ttl: 10s\n    zones:\n"
                                     "    - name: disk_cache_one\n      memory_size: 50m\n      disk_size: 1G\n"
                                     "      disk_path: /tmp/disk_cache_one\n      cache_levels: \"1:2\"\n")
    upstream = {"type": "roundrobin", "nodes": {f"upstream:{server.server_port}": 1}}
    # The committed blocks of the three data-host routes, at their committed priorities; one
    # upstream copy each (a shared dict dumps as a YAML alias, and APISIX then drops the routes).
    # Each route keeps its chain's proxy-buffering through plugin_config_id, as on dev: without it
    # this test kept answers dev never stored (T-6185).
    chains = {route["pluginConfig"] for route in (ROUTE, HISTORY, BULK, MCP, SCHEMA)}
    rules = {"plugin_configs": [{"id": chain, "plugins": {"proxy-buffering": dict(PLUGINS[chain]["plugins"]["proxy-buffering"])}}
                                for chain in sorted(chains)],
             "routes": [{"id": name, "uri": route["uri"], "priority": route["priority"], "upstream": dict(upstream),
                         "plugin_config_id": route["pluginConfig"], "plugins": route.get("plugins", {}),
                         **({"vars": route["vars"]} if "vars" in route else {})}
                        for name, route in (("reads", ROUTE), ("history", HISTORY), ("bulk", BULK), ("mcp", MCP),
                                                 ("schema", SCHEMA))]}
    (tmp / "apisix.yaml").write_text(yaml.safe_dump(rules) + "#END\n")
    port = free_port()
    name = f"apisix-api-cache-{port}"
    started = subprocess.run(
        ["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:9080",
         "--add-host", "upstream:host-gateway",
         "-v", f"{tmp / 'config.yaml'}:/usr/local/apisix/conf/config.yaml:ro",
         "-v", f"{tmp / 'apisix.yaml'}:/usr/local/apisix/conf/apisix.yaml:ro", pinned_image()],
        capture_output=True, text=True, check=False)
    if started.returncode != 0:
        server.shutdown()
        pytest.skip(f"the pinned image does not run here: {started.stderr.strip()}")
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                urllib.request.urlopen(base + "/api/endpoint/ready", timeout=2)
                break
            except OSError:
                time.sleep(0.5)
        else:
            pytest.skip("APISIX or its upstream did not answer within 60 s")
        yield base
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
        server.shutdown()


def get(url, **headers):
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=10) as answer:
        return json.loads(answer.read()), answer.headers


def test_an_anonymous_read_is_kept_and_a_token_or_a_session_is_never_served_from_it(edge):
    url = edge + "/api/endpoint/s/ngsi-ld/v1/entities?type=A"
    first, _ = get(url)
    again, headers = get(url)
    assert again == first and headers["Apisix-Cache-Status"] == "HIT"
    with_token, _ = get(url, Authorization="Bearer person")
    assert with_token["n"] != first["n"] and with_token["auth"] == "Bearer person"
    with_cookie, _ = get(url, Cookie="session=person")
    assert with_cookie["n"] != first["n"]
    # Neither filled the cache with their answer.
    assert get(url)[0] == first


def test_a_header_the_gateway_varies_by_is_its_own_entry(edge):
    url = edge + "/api/endpoint/s/ngsi-ld/v1/entities?type=B"
    english, _ = get(url, **{"Accept-Language": "en"})
    slovak, _ = get(url, **{"Accept-Language": "sk"})
    assert english["n"] != slovak["n"] and slovak["lang"] == "sk"
    assert get(url, **{"Accept-Language": "sk"})[0] == slovak


def test_an_answer_kept_private_is_never_stored(edge):
    # A token's answer is `private, no-store`: it never comes back, not even to the same token.
    url = edge + "/api/endpoint/s/ngsi-ld/v1/entities?type=C"
    one, _ = get(url, Authorization="Bearer a")
    two, headers = get(url, Authorization="Bearer a")
    assert one["n"] != two["n"] and headers.get("Apisix-Cache-Status") != "HIT"


def test_a_read_with_a_link_context_neither_reads_nor_fills_the_cache(edge):
    # T-5900 review: Link names the @context a compacted answer is written in; it is not in the
    # key, so a Link read must never be served a plain answer nor leave its own for a plain read.
    url = edge + "/api/endpoint/s/ngsi-ld/v1/entities?type=D"
    context = '<https://example.org/ctx.jsonld>; rel="http://www.w3.org/ns/json-ld#context"'
    linked, headers = get(url, Link=context)
    assert linked["link"] == context and headers.get("Apisix-Cache-Status") != "HIT"
    plain, _ = get(url)
    assert plain["n"] != linked["n"] and plain["link"] is None
    again, headers = get(url, Link=context)
    assert again["n"] not in (linked["n"], plain["n"]) and headers.get("Apisix-Cache-Status") != "HIT"
    assert get(url)[0] == plain


def test_an_anonymous_history_read_is_kept_and_a_token_or_a_session_is_never_served_from_it(edge):
    url = edge + "/api/endpoint/s/ngsi-ld/v1/temporal/entities?type=A&timerel=after&timeAt=2026-10-31T06:00:00Z"
    first, _ = get(url)
    again, headers = get(url)
    assert again == first and headers["Apisix-Cache-Status"] == "HIT"
    with_token, headers = get(url, Authorization="Bearer person")
    assert with_token["n"] != first["n"] and headers.get("Apisix-Cache-Status") != "HIT"
    with_cookie, headers = get(url, Cookie="session=person")
    assert with_cookie["n"] != first["n"] and headers.get("Apisix-Cache-Status") != "HIT"
    assert get(url)[0] == first


def test_an_export_is_never_stored_although_the_gateway_marks_it_public(edge):
    # The gateway marks an anonymous file.* export `public, max-age=N` as it does a read (T-6185).
    url = edge + "/api/endpoint/s/file.csv?type=A"
    one, _ = get(url)
    two, headers = get(url)
    assert one["n"] != two["n"] and headers.get("Apisix-Cache-Status") is None


def test_an_mcp_stream_reaches_the_client_before_it_ends(edge):
    # T-6185 review: on the buffered reads route nginx held the first event until the upstream closed.
    request = urllib.request.Request(edge + "/api/endpoint/s/mcp", data=b"{}", method="POST",
                                     headers={"Content-Type": "application/json",
                                              "Accept": "application/json, text/event-stream"})
    started = time.monotonic()
    with urllib.request.urlopen(request, timeout=10) as answer:
        assert answer.headers["Content-Type"].startswith("text/event-stream")
        assert answer.headers.get("Apisix-Cache-Status") is None
        first = answer.readline()
        waited = time.monotonic() - started
        assert first == b"event: message\n" and waited < STREAM_PAUSE / 2, (first, waited)
        assert b'"last": true' in answer.read()


def test_a_schema_artifact_read_with_its_etag_is_the_gateways_304(edge):
    # T-6507, EP-51: nginx drops If-None-Match on the way to the upstream in any location with
    # proxy_cache, so on the reads route the reader downloaded the artifact again.
    url = edge + "/api/endpoint/s/schema/v1/linkml"
    with urllib.request.urlopen(url, timeout=10) as answer:
        assert answer.status == 200 and answer.read() == b"id: model\n"
        tag = answer.headers["ETag"]
        assert answer.headers["Cache-Control"] == "private, no-cache"
    request = urllib.request.Request(url, headers={"If-None-Match": tag})
    with pytest.raises(urllib.error.HTTPError) as second:
        urllib.request.urlopen(request, timeout=10)
    assert second.value.code == 304
