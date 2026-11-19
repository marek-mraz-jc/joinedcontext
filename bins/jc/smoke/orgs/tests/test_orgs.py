"""Suite `orgs` and its watch (T-7309): rows from jc.toml and the registry snapshot, the live guard
around them, one run against a loopback edge, the 7-day line log and the incidents.

    python3 -m pytest bins/jc/smoke/orgs/tests -q
"""

import datetime as dt
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import common
import live
import suite as orgs
import watch

TOML = """
mode = "host"
base_host = "city.test"

[mounts.kaisatsu]
host = "idm.city.test"
path_prefix = ""

[[mounts.portal]]
host = "portal.city.test"
path_prefix = ""

[mounts.assistant]
host = "assistant.city.test"
path_prefix = ""

[[mounts.gateway]]
host = "api.city.test"
path_prefix = ""

[[mounts.gateway]]
host = "portal.city.test"
path_prefix = "/api/mcp"

[[organizations]]
name = "alpha"
catalogue_id = "00000000-0000-7000-8000-0000000000a1"
hosts = { portal = "portal.alpha.test", opendata = "opendata.alpha.test", api = "api.alpha.test", idm = "idm.alpha.test" }

[[organizations]]
name = "beta"
catalogue_id = "00000000-0000-7000-8000-0000000000b1"
hosts = { portal = "portal.beta.test" }

[[organizations]]
name = "gone"
catalogue_id = "00000000-0000-7000-8000-0000000000c1"
hosts = { portal = "portal.gone.test" }

[[dataroom.harvest]]
name = "alpha-nkod"
kind = "nkod-sparql"
url = "http://src.alpha.example:{port}/sparql"
publisher = "a"
publisher_title = "A"
publisher_uri = "https://a.example"
organization = "alpha"
allowed_hosts = ["src.alpha.example"]

[[dataroom.harvest]]
name = "beta-arcgis"
kind = "arcgis-hub"
url = "http://hub.beta.example:{port}/"
publisher = "b"
publisher_title = "B"
publisher_uri = "https://b.example"
organization = "beta"
allowed_hosts = ["hub.beta.example"]

[[dataroom.harvest]]
name = "installation"
kind = "ckan-action"
url = "https://ckan.other.example/"
publisher = "c"
publisher_title = "C"
publisher_uri = "https://c.example"
allowed_hosts = ["ckan.other.example"]
"""

NOW = dt.datetime.now(dt.UTC)


def at(hours: float) -> str:
    return (NOW - dt.timedelta(hours=hours)).strftime("%Y-%m-%dT%H:%M:%SZ")


def snapshot(port: int) -> dict:
    return {
        "organizations": [
            {"name": "alpha", "host": "portal.alpha.test", "state": "active", "catalogueId": "a1", "hosts": []},
            {"name": "beta", "host": "portal.beta.test", "state": "active", "catalogueId": "b1",
             "hosts": [{"host": "data.beta.test", "role": "opendata"}]},
            {"name": "gone", "host": "portal.gone.test", "state": "removed", "hosts": []},
            {"name": "gamma", "host": "portal.gamma.test", "state": "active", "hosts": []},
        ],
        "knowledge": [
            {"organization": "alpha", "project": "web", "source": "site", "root": f"http://www.alpha.example:{port}/",
             "lastCrawl": at(2), "pending": 3, "errors": 0},
            {"organization": "beta", "project": "web", "source": "site", "root": f"http://www.beta.example:{port}/",
             "lastCrawl": at(100), "pending": 0, "errors": 7},
            {"organization": "beta", "project": "files", "source": "upload", "root": None, "lastCrawl": None,
             "pending": 0, "errors": 0},
        ],
    }


def test_rows_follow_each_organizations_own_configuration():
    doc = orgs.tomllib.loads(TOML.replace("{port}", "1"))
    got = orgs.build(doc, snapshot(1), {"alpha": ["alpha-chat"]}, {"beta": ["shop.apps.beta.test"]})
    assert sorted(got) == ["alpha", "beta", "gamma"]  # gamma from the registry; gone is removed there
    a, b = got["alpha"], got["beta"]
    assert a["hosts"] == {"portal": "portal.alpha.test", "opendata": "opendata.alpha.test",
                          "api": "api.alpha.test", "idm": "idm.alpha.test"}
    assert b["hosts"] == {"portal": "portal.beta.test", "opendata": "data.beta.test"}
    assert [s["name"] for s in a["harvest"]] == ["alpha-nkod"] and [s["name"] for s in b["harvest"]] == ["beta-arcgis"]
    assert [c["root"] for c in a["crawl"]] == ["http://www.alpha.example:1/"]
    assert [c["source"] for c in b["crawl"]] == ["site", "upload"]
    assert (a["assistant"], b["assistant"]) == ("alpha-chat", None)
    assert b["apps"] == ["shop.apps.beta.test"] and a["apps"] == []
    # The guard admits HEAD to exactly the sources the configuration names, nothing of the installation's.
    assert orgs.outside_hosts(got) == {"src.alpha.example", "hub.beta.example", "www.alpha.example", "www.beta.example"}
    assert "shop.apps.beta.test" in orgs.org_hosts(got)


def test_pairs_take_only_org_equals_value():
    assert orgs.pairs(["alpha=X", "alpha=y"], "--app-host") == {"alpha": ["x", "y"]}
    for bad in ("alpha", "=x", "Alpha=x", "alpha="):
        with pytest.raises(SystemExit):
            orgs.pairs([bad], "--app-host")


@pytest.fixture
def edge():
    """A loopback edge answering as each Organization's jc would; it records what reached it."""
    seen = []

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def reply(self, st, body=b"", ctype="application/json"):
            self.send_response(st)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def do(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            host, path = self.headers.get("Host", "").split(":")[0], self.path
            seen.append((self.command, host, path))
            j = lambda d: self.reply(200, json.dumps(d).encode())
            if path == "/realms/alpha/.well-known/openid-configuration" and host == "idm.alpha.test":
                return j({"issuer": "http://idm.alpha.test/realms/alpha"})
            if path == "/realms/jc/.well-known/openid-configuration" and host == "idm.city.test":
                return j({"issuer": "http://idm.city.test/realms/jc"})
            if path == "/.well-known/oauth-protected-resource/api/mcp":
                if host != "api.city.test":  # an Organization host answers the SPA here
                    return self.reply(200, b"<!doctype html>", "text/html")
                return j({"resource": "http://api.city.test/api/mcp",
                          "authorization_servers": ["http://api.city.test", "http://idm.city.test/realms/jc"]})
            if path == "/.well-known/oauth-protected-resource/api/v1/mcp":
                # alpha: its own realm; beta: another realm; gamma: its realm, but another host's resource.
                realm = {"portal.alpha.test": "http://idm.alpha.test/realms/alpha",
                         "portal.beta.test": "http://idm.city.test/realms/alpha"}.get(host, "http://idm.city.test/realms/jc")
                res = "http://api.city.test/api/v1/mcp" if host == "portal.gamma.test" else f"http://{host}/api/v1/mcp"
                return j({"resource": res, "authorization_servers": [realm]})
            if path.endswith("/api/3/action/package_search?rows=0"):
                return j({"success": True})
            if path == "/api/mcp":
                msg = json.loads(raw)
                result = {"tools": [{"name": "query_entities"}]} if msg["method"] == "tools/list" else {"protocolVersion": "x"}
                return j({"jsonrpc": "2.0", "id": msg["id"], "result": result})
            if path == "/api/v1/d/alpha-chat/challenge":
                return j({"challenge": "c.1.4.s", "difficulty": 4})
            if path == "/api/v1/d/alpha-chat/token":
                return j({"token": "t"})
            if path == "/api/v1/d/alpha-chat/chat" and self.headers.get("X-JC-Visitor") == "t":
                sse = (b'event: answer\ndata: {"text": "x"}\n\n'
                       b'event: citations\ndata: [{"n": 1, "url": "https://www.alpha.example/about"}]\n\n'
                       b'event: done\ndata: {}\n\n')
                return self.reply(200, sse, "text/event-stream")
            if host.startswith("portal.") and path == "/":
                return self.reply(302)
            if host in ("src.alpha.example", "www.alpha.example", "www.beta.example"):
                return self.reply(405 if self.command == "HEAD" and host == "src.alpha.example" else 200)
            if host == "hub.beta.example":
                return self.reply(503)
            return self.reply(404)

        do_GET = do_POST = do_HEAD = do_PUT = do_DELETE = do

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port, seen
    srv.shutdown()


@pytest.fixture
def public_sources(monkeypatch, edge):
    """The outside hosts resolve to the loopback edge and count as public there."""
    port = edge[0]
    monkeypatch.setattr(live, "resolve", lambda host, p: [("127.0.0.1", p)])
    monkeypatch.setattr(live, "is_public", lambda ip: True)
    return port


def run(tmp_path, port, *extra, snap_doc=None):
    toml = tmp_path / "jc.toml"
    toml.write_text(TOML.replace("{port}", str(port)))
    snap = tmp_path / "snap.json"
    snap.write_text(json.dumps(snap_doc or snapshot(port)))
    out = tmp_path / "out"
    code = orgs.main(["--out", str(out), "--jc-config", str(toml), "--deployment", "t", "--domain", "city.test",
                      "--edge", "127.0.0.1", "--edge-port", str(port), "--scheme", "http", "--rate", "1000",
                      "--snapshot", str(snap), "--assistant", "alpha=alpha-chat", *extra])
    assert live.active() is None
    return code, json.loads((out / "results.json").read_text())


def test_one_run_probes_each_organization_and_names_what_is_red(tmp_path, edge, public_sources):
    code, res = run(tmp_path, public_sources, "--app-host", "alpha=nosuch.apps.alpha.test")
    rows = {r["id"]: r for r in res["checks"]}
    green = ["prm.gateway", "portal.alpha", "oidc.alpha", "prm.alpha", "catalogue.alpha", "mcp.alpha", "harvest.alpha.alpha-nkod",
             "crawl.alpha.web/site", "crawl.alpha.web/site.fresh", "embed.alpha", "assistant.alpha",
             "portal.beta", "oidc.beta", "catalogue.beta", "crawl.beta.web/site"]
    assert {k: rows[k]["result"] for k in green} == {k: "pass" for k in green}, rows
    red = {k for k, r in rows.items() if r["result"] == "fail"}
    # beta: the installation realm's metadata names realm alpha; its harvest source answers 503;
    # its last crawl is 100 h old; the embedder refused 7 passages. alpha: the App host is unknown.
    # gamma: its metadata names api.city.test as the resource.
    assert rows["prm.gamma"]["actual"] == "200 resource api.city.test http://idm.city.test/realms/jc"
    assert rows["prm.alpha"]["call"] == "GET portal.alpha.test/.well-known/oauth-protected-resource/api/v1/mcp"
    # The gateway's /api/mcp metadata is asked of the gateway host only, never of an Organization host.
    assert [h for m, h, p in edge[1] if p == "/.well-known/oauth-protected-resource/api/mcp"] == ["api.city.test"]
    assert {"prm.beta", "prm.gamma", "harvest.beta.beta-arcgis", "crawl.beta.web/site.fresh", "embed.beta",
            "app.alpha.nosuch.apps.alpha.test"} <= red
    assert rows["embed.beta"]["actual"] == "7 refused, 0 waiting"
    assert rows["crawl.beta.files/upload"]["result"] == "skip"
    assert rows["assistant.beta"]["result"] == "skip" and "--assistant beta=" in rows["assistant.beta"]["reason"]
    assert code == 1
    assert res["organizations"]["alpha"]["fail"] == 1 and res["organizations"]["beta"]["fail"] >= 4
    # No answer body reaches a row: statuses and counts only.
    assert not any("query_entities" in r["actual"] or "issuer" in r["actual"] for r in res["checks"])


def test_an_assistant_schema_before_migration_0025_skips_the_embed_row(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    for k in doc["knowledge"]:
        k["errors"] = None  # watch.assistant_sql(False): the column does not exist there
    _, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {r["id"]: r for r in res["checks"]}
    assert rows["embed.alpha"]["result"] == rows["embed.beta"]["result"] == "skip"
    assert "migration 0025" in rows["embed.beta"]["reason"]


def test_the_guard_refuses_writes_foreign_hosts_and_private_sources(tmp_path, edge, monkeypatch):
    port, seen = edge
    toml = TOML.replace("{port}", str(port))
    got = orgs.build(orgs.tomllib.loads(toml), snapshot(port))
    cfg = live.config("t", "city.test", "127.0.0.1", toml, tmp_path, rate=1000, edge_port=port, scheme="http",
                      org_hosts=orgs.org_hosts(got), outside=orgs.outside_hosts(got))
    live.install(cfg)
    try:
        org = common.Http("http://portal.alpha.test", timeout=5)
        src = common.Http(f"http://www.alpha.example:{port}", timeout=5)
        refused = [
            org.call("POST", "/api/v1/projects", body={"name": "x"}),  # a write
            org.call("DELETE", "/api/v1/projects/x"),
            org.call("GET", "/api/v1/auth/me", token="a.b.c"),  # a credential
            src.call("GET", "/"),  # an outside host takes HEAD only
            src.call("POST", "/", body={}),
            common.Http("http://ckan.other.example", timeout=5).call("HEAD", "/"),  # not an Organization's
            common.Http("http://evil.example", timeout=5).call("HEAD", "/"),
        ]
        assert all(st == 0 and b"jc-smoke live guard refused" in raw for st, _, raw in refused), refused
        # A source whose DNS names a private or loopback address is never reached.
        assert src.call("HEAD", "/")[0] == 0 and seen == []
        monkeypatch.setattr(live, "resolve", lambda host, p: [("10.0.0.5", p)])
        st, _, raw = src.call("HEAD", "/")
        assert st == 0 and b"not public" in raw
        monkeypatch.setattr(live, "resolve", lambda host, p: [("127.0.0.1", p)])
        monkeypatch.setattr(live, "is_public", lambda ip: True)
        assert src.call("HEAD", "/")[0] == 200
        assert org.call("POST", "/api/v1/d/alpha-chat/chat", body={"message": "q"})[0] == 404  # the visitor door
    finally:
        live.uninstall()
    assert seen == [("HEAD", "www.alpha.example", "/"), ("POST", "portal.alpha.test", "/api/v1/d/alpha-chat/chat")]


class FakeBoard(watch.Board):
    def __init__(self):
        super().__init__(None, "jc-dev", "deployer")
        self.opened, self.closed = [], []

    def open(self, key, row, at):
        self.opened.append(key)
        return f"T-{len(self.opened)}"

    def close(self, tid, key, why, at):
        self.closed.append((tid, key, why))


def results(**rows):
    return {"totals": {}, "checks": [{"id": k, "target": k.split(".")[1], "result": v} for k, v in rows.items()]}


def test_incidents_open_once_per_organization_and_check_and_close_when_green():
    b = FakeBoard()
    state = watch.incidents({}, results(**{"app.alpha.x": "fail", "embed.beta": "fail", "portal.alpha": "pass"}),
                            True, None, b, "t1")
    assert state == {"app.alpha.x": "T-1", "embed.beta": "T-2"}
    state = watch.incidents(state, results(**{"app.alpha.x": "fail", "embed.beta": "fail"}), True, None, b, "t2")
    assert b.opened == ["app.alpha.x", "embed.beta"]  # still red: no second task
    # An incomplete run closes only what passed: the App row it did not run stays open.
    state = watch.incidents(state, results(**{"embed.beta": "pass"}), False, "snapshot: kubectl exec: denied", b, "t3")
    assert state == {"app.alpha.x": "T-1", "snapshot": "T-3"} and b.closed == [("T-2", "embed.beta", "passes")]
    # A complete run no longer probing the App host (the deliberate red removed) closes it.
    state = watch.incidents(state, results(**{"portal.alpha": "pass"}), True, None, b, "t4")
    assert state == {} and ("T-1", "app.alpha.x", "is no longer probed") in b.closed


def test_the_line_names_each_organization_and_the_log_keeps_seven_days(tmp_path):
    now = dt.datetime(2026, 11, 19, 12, 0, tzinfo=dt.UTC)
    res = {"totals": {"pass": 10, "fail": 2, "skip": 1}, "organizations": {"beta": {"checks": 6, "fail": 2},
           "alpha": {"checks": 7, "fail": 0}},
           "checks": [{"id": "embed.beta", "result": "fail"}, {"id": "prm.beta", "result": "fail"}]}
    line = watch.summary("jc-dev", now, res, None)
    assert line == "2026-11-19T12:00:00Z jc-dev: 13 checks, 2 fail; alpha 0/7, beta 2/6; red embed.beta prm.beta"
    assert watch.summary("jc-dev", now, None, "snapshot: x") == "2026-11-19T12:00:00Z jc-dev: no result (snapshot: x)"
    log = tmp_path / "orgs.log"
    log.write_text("2026-11-10T00:00:00Z jc-dev: old\n2026-11-15T00:00:00Z jc-dev: kept\n")
    watch.keep_week(log, line, now)
    assert log.read_text().splitlines() == ["2026-11-15T00:00:00Z jc-dev: kept", line]
    runs = tmp_path / "runs"
    for name in ("20261110T000000Z", "20261118T000000Z"):
        (runs / name).mkdir(parents=True)
    watch.prune_runs(runs, now)
    assert [d.name for d in runs.iterdir()] == ["20261118T000000Z"]


@pytest.mark.parametrize("embed_error", [True, False])
def test_the_snapshot_reads_only_inside_a_read_only_transaction(monkeypatch, tmp_path, embed_error):
    calls = []

    def fake(cfg, *args, timeout=60):
        calls.append(args)
        if args[-1] == watch.EMBED_ERROR_SQL:
            return "SET\n" + json.dumps({"embedError": embed_error})
        if args[-1].startswith("SELECT"):
            return "SET\n" + json.dumps({"organizations": []} if "organizations" in args[-1] else {"knowledge": []})
        return "pod/postgres-cluster-1\n" if "pod" in args else 'mode = "host"\n'

    monkeypatch.setattr(watch, "kubectl", fake)
    cfg = {"namespace": "jc", "jc_configmap": "jc", "pg_namespace": "pg", "pg_cluster": "postgres-cluster"}
    toml, snap = watch.snapshot(cfg, tmp_path)
    # jc.toml comes from the ConfigMap: the jc image has no shell tools to exec
    assert calls[0] == ("-n", "jc", "get", "configmap", "jc", "-o", r"jsonpath={.data.jc\.toml}")
    assert not any("exec" in c and c[1] == "jc" for c in calls)
    assert toml.read_text() == 'mode = "host"\n'
    assert json.loads(snap.read_text()) == {"organizations": [], "knowledge": []}
    queries = [c for c in calls if c[-1].startswith("SELECT")]
    assert len(queries) == 3 and all("SET default_transaction_read_only = on" in c for c in queries)
    # Before assistant migration 0025 the query names no chunks.embed_error (the column does not exist).
    assert ("embed_error" in queries[-1][-1]) is embed_error
    for sql in (watch.PORTAL_SQL, watch.assistant_sql(True), watch.assistant_sql(False), watch.EMBED_ERROR_SQL):
        assert not any(w in sql.upper() for w in ("INSERT", "UPDATE ", "DELETE", "DROP", "TRUNCATE", "C.TEXT"))
