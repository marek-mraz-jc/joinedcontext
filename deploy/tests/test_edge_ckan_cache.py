"""T-4717 step 4: the edge keeps CKAN's anonymous action API reads 60 s, and nothing private.

The first tests read the committed route and chart. The last runs the pinned APISIX image with
the committed `proxy-cache` block in front of a local upstream, because only answers prove what
nginx stores: a person's session (any cookie), a token's view (Authorization), an answer CKAN
marks private and one that sets a cookie must never come back to someone else.
"""

import http.server
import itertools
import json
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
from pathlib import Path

import pytest
import yaml

from test_apisix_429_contract import pinned_image

pytestmark = pytest.mark.xdist_group("docker-apisix-cache")

ROOT = Path(__file__).resolve().parent.parent
ROUTE = yaml.safe_load((ROOT / "components/ckan/apisix-routes.yaml").read_text())["ckan-action-cache"]


def test_the_cache_route_is_the_action_api_on_the_catalogue_chain():
    assert (ROUTE["subDomain"], ROUTE["uri"], ROUTE["pluginConfig"]) == ("opendata", "/api/3/action/*", "ckan")
    assert ROUTE["methods"] == ["GET", "HEAD"]
    cache = ROUTE["plugins"]["proxy-cache"]
    assert cache["cache_strategy"] == "disk" and cache["cache_ttl"] == 60
    for field in ("cache_bypass", "no_cache"):
        # CKAN reads a token from Authorization and from the legacy X-CKAN-API-Key.
        assert set(cache[field]) >= {"$http_cookie", "$http_authorization", "$http_x_ckan_api_key"}, field
    assert cache["cache_http_status"] == [200] and cache["cache_method"] == ["GET", "HEAD"]


def test_ckan_marks_an_anonymous_api_answer_public_for_sixty_seconds():
    template = (ROOT / "charts/ckan/templates/ckan.yaml").read_text()
    assert '- name: CKAN__CACHE_EXPIRES\n              value: "60"' in template


class Upstream(http.server.BaseHTTPRequestHandler):
    counter = itertools.count()

    def do_GET(self):
        n = next(self.counter)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        if self.path.startswith("/api/3/action/private"):
            self.send_header("Cache-Control", "private")
        else:
            self.send_header("Cache-Control", "public, max-age=60, must-revalidate")
        if self.path.startswith("/api/3/action/session"):
            self.send_header("Set-Cookie", f"ckan=session-{n}; Path=/; HttpOnly")
        if self.headers.get("Origin"):
            self.send_header("Access-Control-Allow-Origin", self.headers["Origin"])
        self.end_headers()
        self.wfile.write(json.dumps({"n": n, "cookie": self.headers.get("Cookie")}).encode())

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
    server = http.server.ThreadingHTTPServer(("0.0.0.0", 0), Upstream)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    tmp = tmp_path_factory.mktemp("cache")
    (tmp / "config.yaml").write_text("deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n")
    rules = {"routes": [{"id": "r", "uri": "/api/3/action/*", "methods": ["GET", "HEAD"],
                         "upstream": {"type": "roundrobin", "nodes": {f"upstream:{server.server_port}": 1}},
                         "plugins": {"proxy-cache": ROUTE["plugins"]["proxy-cache"]}}]}
    (tmp / "apisix.yaml").write_text(yaml.safe_dump(rules) + "#END\n")
    port = free_port()
    name = f"apisix-cache-{port}"
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
                urllib.request.urlopen(base + "/api/3/action/ready", timeout=2)
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


def test_an_anonymous_read_is_kept_and_a_session_or_a_token_is_never_served_from_it(edge):
    first, _ = get(edge + "/api/3/action/package_show?id=a")
    again, headers = get(edge + "/api/3/action/package_show?id=a")
    assert again == first and headers["Apisix-Cache-Status"] == "HIT"
    with_cookie, _ = get(edge + "/api/3/action/package_show?id=a", Cookie="ckan=person")
    assert with_cookie["n"] != first["n"] and with_cookie["cookie"] == "ckan=person"
    with_token, _ = get(edge + "/api/3/action/package_show?id=a", Authorization="token")
    assert with_token["n"] != first["n"]
    with_key, _ = get(edge + "/api/3/action/package_show?id=a", **{"X-CKAN-API-Key": "token"})
    assert with_key["n"] != first["n"]
    # Neither filled the cache with their answer.
    assert get(edge + "/api/3/action/package_show?id=a")[0] == first


def test_an_answer_ckan_marks_private_or_one_that_sets_a_cookie_is_never_kept(edge):
    for path in ("/api/3/action/private_show", "/api/3/action/session_show"):
        one, _ = get(edge + path)
        two, headers = get(edge + path)
        assert one["n"] != two["n"], path
        assert headers.get("Apisix-Cache-Status") != "HIT", path


def test_an_answer_kept_for_one_origin_is_never_served_to_another(edge):
    # CKAN names the asking Origin in Access-Control-Allow-Origin, so each Origin keeps its own copy.
    _, bare = get(edge + "/api/3/action/status_show")
    assert "Access-Control-Allow-Origin" not in bare
    _, viz = get(edge + "/api/3/action/status_show", Origin="https://viz.example")
    assert viz["Access-Control-Allow-Origin"] == "https://viz.example"
    _, other = get(edge + "/api/3/action/status_show", Origin="https://other.example")
    assert other["Access-Control-Allow-Origin"] == "https://other.example"
