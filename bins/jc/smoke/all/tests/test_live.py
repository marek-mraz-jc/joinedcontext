"""Live mode (live.py): the host map, the process-wide guard, the shared rate cap, and `run all
--deployment`. Every request goes to a stub on loopback that stands in for the edge.

    python3 -m pytest bins/jc/smoke/all/tests/test_live.py -q
"""

import json
import os
import shutil
import socket
import ssl
import subprocess
import sys
import threading
import time
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import suite as all_suite  # noqa: E402
import common  # noqa: E402
import live  # noqa: E402

# deployment main's host-mode mounts (components/jc/values/server/base-values.yaml.gotmpl), cut down.
TOML = """
mode = "host"
base_host = "city.test"

[mounts.kaisatsu]
host = "idm.city.test"
path_prefix = ""

[[mounts.portal]]
host = "portal.city.test"
path_prefix = ""

[[mounts.portal]]
host = "*.apps.city.test"

[mounts.dataroom]
host = "opendata.city.test"
path_prefix = ""

[[mounts.shako]]
host = "localhost:8080"
path_prefix = "/git"

[mounts.viz]
host = "viz.evil.example"
path_prefix = ""

[[mounts.gateway]]
host = "api.city.test"
path_prefix = ""

[[mounts.gateway]]
host = "city.test"
path_prefix = "/api/mcp"
"""


@pytest.fixture
def edge():
    """A stub edge on loopback: answers 200 and counts what reached it."""
    seen = []

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do(self):
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            seen.append((self.command, self.headers.get("Host"), self.path, self.headers.get("Authorization")))
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"{}")

        do_GET = do_POST = do_DELETE = do_HEAD = do

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port, seen
    srv.shutdown()


@pytest.fixture
def guard(tmp_path, edge):
    port, seen = edge
    cfg = live.config("t", "city.test", "127.0.0.1", TOML, tmp_path, rate=1000, edge_port=port, scheme="http")
    g = live.install(cfg)
    try:
        yield g, seen
    finally:
        live.uninstall()


def test_paths_of_the_rehearsal_go_to_the_host_jc_serves_them_on(tmp_path):
    g = live.Guard(live.config("t", "city.test", "127.0.0.1", TOML, tmp_path))
    v = "t.smoke.invalid"
    assert g.route(v, "/id/realms/smartcity?x=1") == ("idm.city.test", "/realms/smartcity?x=1")
    assert g.route(v, "/id") == ("idm.city.test", "/")
    assert g.route(v, "/opendata/api/3/action/package_list") == ("opendata.city.test", "/api/3/action/package_list")
    assert g.route(v, "/api/endpoint/abc/ngsi-ld/v1/types") == ("api.city.test", "/api/endpoint/abc/ngsi-ld/v1/types")
    assert g.route(v, "/api/mcp") == ("api.city.test", "/api/mcp")
    assert g.route(v, "/api/v1/projects") == ("portal.city.test", "/api/v1/projects")
    assert g.route(v, "/identity") == ("portal.city.test", "/identity")  # whole segments only
    # An absolute address on a host of the deployment stays on it.
    assert g.route("x", "https://opendata.city.test/dataset/a") == ("opendata.city.test", "/dataset/a")


def test_a_host_outside_the_domain_is_refused(tmp_path):
    g = live.Guard(live.config("t", "city.test", "127.0.0.1", TOML, tmp_path))
    assert g.hosts == {"idm.city.test", "portal.city.test", "opendata.city.test", "api.city.test", "city.test"}
    for host in ("viz.evil.example", "localhost:8080", "localhost", "a.apps.city.test", "city.test.evil.example"):
        with pytest.raises(live.Refused):
            g.route(host, "/")
    with pytest.raises(SystemExit, match="base_host"):
        live.config("t", "other.test", "127.0.0.1", TOML, tmp_path)
    with pytest.raises(SystemExit, match="host-mode"):
        live.config("t", "city.test", "127.0.0.1", TOML.replace('mode = "host"', 'mode = "slug"'), tmp_path)


def test_only_anonymous_reads_and_the_named_posts_reach_the_edge(guard, tmp_path, edge):
    g, seen = guard
    port = edge[0]
    http = common.Http("http://t.smoke.invalid", timeout=5)
    assert http.call("GET", "/id/realms/smartcity")[0] == 200
    assert http.call("POST", "/api/mcp", body={"jsonrpc": "2.0", "id": 1, "method": "initialize"})[0] == 200
    assert http.call("POST", "/api/endpoint/abc/ngsi-ld/v1/entities", body={"id": "x", "type": "T"})[0] == 200
    refused = [
        http.call("POST", "/api/v1/projects", body={"name": "x"}),  # a write row
        http.call("POST", "/api/mcp", token="a.b.c", body={}),  # a POST with a token
        http.call("GET", "/api/v1/auth/me", token="a.b.c"),
        http.call("GET", "/api/v1/projects", headers={"Cookie": "s=1"}),
        http.call("DELETE", "/api/endpoint/abc/ngsi-ld/v1/entities/x"),
        http.call("PATCH", "/api/endpoint/abc/ngsi-ld/v1/entities/x/attrs", body={}),
        common.Http("http://example.org", timeout=5).call("GET", "/"),
        common.Http(f"http://127.0.0.1:{port}", timeout=5).call("GET", "/"),
    ]
    assert all(st == 0 and b"jc-smoke live guard refused" in raw for st, _, raw in refused), refused
    for address in (("127.0.0.1", port), ("api.city.test", 443), ("example.org", 80)):
        with pytest.raises(live.Refused):
            socket.create_connection(address, timeout=2)  # a raw socket, even to the edge
    with pytest.raises(live.Refused):
        socket.socket().connect(("127.0.0.1", port))
    with pytest.raises(live.Refused):
        subprocess.run(["curl", f"http://127.0.0.1:{port}/"])
    assert seen == [("GET", "idm.city.test", "/realms/smartcity", None),
                    ("POST", "api.city.test", "/api/mcp", None),
                    ("POST", "api.city.test", "/api/endpoint/abc/ngsi-ld/v1/entities", None)], seen
    log = (tmp_path / "refused.jsonl").read_text().splitlines()
    assert len(log) == len(refused) + 5 and "write row" in json.loads(log[0])["why"]


def test_tls_stays_verified_and_https_is_the_only_scheme(tmp_path, edge):
    port, seen = edge
    live.install(live.config("t", "city.test", "127.0.0.1", TOML, tmp_path, edge_port=port))
    try:
        c = http.client.HTTPSConnection("api.city.test", context=ssl._create_unverified_context(), timeout=2)
        with pytest.raises(live.Refused, match="verification"):
            c.request("GET", "/api/endpoints")
        with pytest.raises(live.Refused, match="https only"):
            http.client.HTTPConnection("api.city.test", timeout=2).request("GET", "/api/endpoints")
    finally:
        live.uninstall()
    assert seen == []


def test_the_rate_cap_holds_across_threads_and_processes(tmp_path):
    cfg = live.config("t", "city.test", "127.0.0.1", TOML, tmp_path, rate=20)
    child = subprocess.Popen([sys.executable, "-c", (
        "import json, sys, time; sys.path.insert(0, sys.argv[1]); import live\n"
        "g = live.Guard(json.loads(sys.argv[2]))\n"
        "for _ in range(4):\n    print(g.wait())\n"), str(HERE.parent.parent), json.dumps(cfg)],
        stdout=subprocess.PIPE, text=True)
    g, times, lock = live.Guard(cfg), [], threading.Lock()

    def waiter():
        for _ in range(4):
            t = g.wait()  # the grant time, read under the bucket lock
            with lock:
                times.append(t)

    threads = [threading.Thread(target=waiter) for _ in range(3)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    times += [float(x) for x in child.communicate()[0].split()]
    times.sort()
    gaps = [b - a for a, b in zip(times, times[1:])]
    assert len(times) == 16 and min(gaps) >= 0.049, gaps  # 20/s: one grant each 50 ms


def test_loopback_only_takes_the_virtual_base_only_with_the_guard_installed(guard):
    common.loopback_only("http://t.smoke.invalid")
    for base in ("https://api.dev.joinedcontext.com", "https://u.smoke.invalid"):
        with pytest.raises(SystemExit, match="refusing"):
            common.loopback_only(base)
    live.uninstall()
    with pytest.raises(SystemExit, match="refusing"):
        common.loopback_only("http://t.smoke.invalid")


def test_redact_keeps_no_person_data():
    body = '{"email":"jana.novak@city.cz","username":"jnovak","firstName":"Jana","sub":"9f1c","title":"Air"}'
    out = live.redact(body + " mail petr@x.org")
    assert "novak" not in out and "Jana" not in out and "9f1c" not in out and "petr" not in out
    assert '"title":"Air"' in out


NAMES = '''
import argparse
from pathlib import Path
from common import Results, loopback_only
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out"); p.add_argument("--export", required=True)
    p.add_argument("--state")
    a, rest = p.parse_known_args(argv)
    loopback_only(a.base_url)
    r = Results("names", a.base_url)
    r.check("base", "t", "argv", "the virtual base, no state", f"{a.base_url} {a.state} {rest}",
            a.base_url == "https://t.smoke.invalid" and a.state is None)
    r.write(Path(a.out))
    return 0
'''


def test_run_all_live_runs_the_cleared_suites_and_skips_the_rest_by_name(tmp_path, monkeypatch):
    root = tmp_path / "smoke"
    root.mkdir()
    for f in ("run", "common.py", "live.py"):
        shutil.copy(all_suite.SMOKE / f, root / f)
    for name, text in (("names", NAMES), ("ui", "def main(argv):\n    raise SystemExit('ran')\n")):
        (root / name).mkdir()
        (root / name / "suite.py").write_text(text)
    monkeypatch.setattr(all_suite, "SMOKE", root)
    monkeypatch.setattr(live, "SMOKE", root)
    monkeypatch.delenv(live.ENV, raising=False)
    (tmp_path / "jc.toml").write_text(TOML)
    common_args = ["--deployment", "t", "--domain", "city.test", "--edge", "127.0.0.1",
                   "--jc-config", str(tmp_path / "jc.toml"), "--export", "E"]
    for extra in (["--state", "S"], ["--admin-token-file", "T"], ["--base-url", "http://localhost:1"], ["--e2e"]):
        with pytest.raises(SystemExit):
            all_suite.main(common_args + ["--out", str(tmp_path / "refused")] + extra)
    out = tmp_path / "out"
    try:
        code = all_suite.main(common_args + ["--out", str(out)])
    finally:
        live.uninstall()
    doc = json.loads((out / "results.json").read_text())
    rows = {r["id"]: r for r in doc["checks"]}
    assert rows["names/base"]["result"] == "pass", rows["names/base"]
    assert rows["ui/run"]["result"] == "skip" and "rehearsal only" in rows["ui/run"]["reason"]
    assert rows["e2e/cargo"]["result"] == "skip"
    assert code == 0 and doc["base"] == "https://t.smoke.invalid"
    assert "Live: deployment t" in (out / "REPORT.md").read_text()
    assert os.environ.get(live.ENV)  # the suites' processes inherit the guard


def test_playwright_and_the_socket_clients_cannot_start_under_the_guard(guard, edge):
    """The ui, multiorg and charts-apps browsers and the subscriptions socket clients are skipped
    by name in live mode; were one to run anyway, the guard stops it before any byte leaves."""
    sys.path.insert(0, str(HERE.parent.parent / "subscriptions"))
    import mqttclient
    import wsclient
    port, seen = edge
    with pytest.raises(live.Refused):
        wsclient.Ws(f"http://api.city.test:{port}", "/api/endpoint/abc/ws")
    with pytest.raises(live.Refused):
        mqttclient.Mqtt(f"mqtt://127.0.0.1:{port}")
    sync_api = pytest.importorskip("playwright.sync_api")
    with pytest.raises(live.Refused, match="exec"):
        sync_api.sync_playwright().start()
    assert seen == []


def test_anonymous_mcp_calls_keep_under_the_gateways_per_address_limit(tmp_path, edge):
    """Behind the edge jc counts anonymous MCP per client address (120 a minute): the guard paces
    every MCP POST at live.MCP_RATE whatever the general rate, an unknown slug's door included."""
    port, seen = edge
    live.install(live.config("t", "city.test", "127.0.0.1", TOML, tmp_path, rate=1000, edge_port=port,
                             scheme="http"))
    try:
        http = common.Http("http://t.smoke.invalid", timeout=5)
        started = time.time()
        for path in ("/api/mcp", "/api/endpoint/smoke-names-0a1b/mcp", "/api/mcp/p/open", "/api/mcp"):
            assert http.call("POST", path, body={})[0] == 200
        took = time.time() - started
    finally:
        live.uninstall()
    assert live.MCP_RATE * 60 < 120 and took >= 3 / live.MCP_RATE - 0.05, took
    assert len(seen) == 4
