"""Suite `orgs` and its watch (T-7309): rows from jc.toml and the registry snapshot, the live guard
around them, one run against a loopback edge, the 7-day line log and the incidents.

    python3 -m pytest bins/jc/smoke/orgs/tests -q
"""

import datetime as dt
import json
import os
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
        # sites.organization holds the site's domain; the project names the Organization.
        "projects": [{"name": "alpha-web", "organization": "alpha"}, {"name": "beta-web", "organization": "beta"},
                     {"name": "beta-files", "organization": "beta"}, {"name": "gone-web", "organization": "gone"}],
        "knowledge": [
            {"domain": "alpha.example", "project": "alpha-web", "source": "site",
             "root": f"http://www.alpha.example:{port}/", "lastCrawl": at(2), "pending": 3, "errors": 0,
             "titles": [{"title": "Parking permits", "lang": "en"}]},
            {"domain": "beta.example", "project": "beta-web", "source": "site", "root": f"http://www.beta.example:{port}/",
             "lastCrawl": at(100), "pending": 0, "errors": 7, "titles": []},
            {"domain": "beta.example", "project": "beta-files", "source": "upload", "root": None, "lastCrawl": None,
             "pending": 0, "errors": 0},
            {"domain": "alpha", "project": "unmapped", "source": "site", "root": "http://www.other.example/",
             "lastCrawl": at(1), "pending": 0, "errors": 0},
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
    # Through the project, never by the domain column: "unmapped" names alpha as its domain and stays out.
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


def test_a_source_belongs_to_the_organization_of_its_project():
    doc = orgs.tomllib.loads(TOML.replace("{port}", "1"))
    snap = {"projects": [{"name": "p1", "organization": "beta"}],
            "knowledge": [{"domain": "alpha", "project": "p1", "source": "s"},
                          {"domain": "beta", "project": "p2", "source": "s"}]}
    got = orgs.build(doc, snap)
    assert [c["project"] for c in got["beta"]["crawl"]] == ["p1"] and got["alpha"]["crawl"] == []


def test_the_route_comes_from_the_done_event_and_only_search_counts():
    assert orgs.route([("done", {"cached": False, "stagesMs": {"route": 9, "search": 40, "answer": 900}})]) == "search"
    assert orgs.route([("done", {"cached": True, "stagesMs": {"route": 3}})]) == "cached"
    assert orgs.route([("done", {"cached": False, "stagesMs": {"route": 9, "answer": 500}})]) == "direct"
    assert orgs.route([("answer", {"text": "x"})]) == "direct"


def test_the_question_names_a_page_title_and_never_repeats_the_previous_one():
    titles = [{"title": "Parkovanie\n v centre", "lang": "sk-SK"}, {"title": "Opening hours", "lang": None},
              {"title": "x", "lang": "en"}, "not a row"]
    rng = orgs.random.Random(7)
    asked = {orgs.question(titles, None, rng) for _ in range(200)}
    assert asked == {q.format(t="Parkovanie v centre") for q in orgs.ASK["sk"]} | \
        {q.format(t="Opening hours") for q in orgs.ASK["en"]}
    prev = orgs.ASK["en"][0].format(t="Opening hours")
    # Another title first; with one title left, its other phrasing; never the previous question.
    assert all("Opening hours" not in orgs.question(titles, prev, rng) for _ in range(100))
    one = [{"title": "Opening hours", "lang": "en"}]
    assert {orgs.question(one, prev, rng) for _ in range(50)} == {orgs.ASK["en"][1].format(t="Opening hours")}
    assert orgs.question([{"title": "  ", "lang": "en"}], None, rng) is None
    assert len(orgs.question([{"title": "a" * 500}], None, rng)) < orgs.TITLE_MAX + 40
    assert orgs.previous_questions(None) == {}


def test_the_question_comes_from_the_projects_the_assistant_searches(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    doc["projects"].append({"name": "alpha-news", "organization": "alpha"})
    doc["knowledge"].append({"domain": "alpha.example", "project": "alpha-news", "source": "site", "root": None,
                             "titles": [{"title": "Election results", "lang": "en"}]})
    run(tmp_path, public_sources, "--assistant", "alpha=alpha-chat@alpha-news", snap_doc=doc)
    asked = [m for k, h, m in edge[1] if k == "ASK"]
    assert len(asked) == 1 and "Election results" in asked[0]
    got = orgs.build(orgs.tomllib.loads(TOML.replace("{port}", "1")), doc, {"alpha": ["alpha-chat@a,b/c"]})
    assert (got["alpha"]["assistant"], got["alpha"]["asks"]) == ("alpha-chat", {"a", "b/c"})
    # A source the deployment does not search gives no question: the row fails by name.
    edge[1].clear()
    _, res = run(tmp_path, public_sources, "--assistant", "alpha=alpha-chat@alpha-web/other", snap_doc=doc)
    assert {r["id"]: r for r in res["checks"]}["assistant.alpha"]["result"] == "fail"
    assert not [m for k, h, m in edge[1] if k == "ASK"]


@pytest.fixture
def edge():
    """A loopback edge answering as each Organization's jc would; it records what reached it."""
    seen = []
    done = {"cached": False, "stagesMs": {"route": 5, "search": 30, "answer": 400}}

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
                seen.append(("ASK", host, json.loads(raw)["message"]))
                sse = (b'event: answer\ndata: {"text": "x"}\n\n'
                       b'event: citations\ndata: [{"n": 1, "url": "https://www.alpha.example/about"}]\n\n'
                       b'event: done\ndata: ' + json.dumps(done).encode() + b'\n\n')
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
    yield srv.server_port, seen, done
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
                      "--snapshot", str(snap), "--assistant", "alpha=alpha-chat",
                      "--manifests", str(tmp_path / "manifests"), *extra])
    assert live.active() is None
    return code, json.loads((out / "results.json").read_text())


def test_one_run_probes_each_organization_and_names_what_is_red(tmp_path, edge, public_sources):
    code, res = run(tmp_path, public_sources, "--app-host", "alpha=nosuch.apps.alpha.test")
    rows = {r["id"]: r for r in res["checks"]}
    green = ["prm.gateway", "portal.alpha", "oidc.alpha", "prm.alpha", "catalogue.alpha", "mcp.alpha", "harvest.alpha.alpha-nkod",
             "crawl.alpha.alpha-web/site", "crawl.alpha.alpha-web/site.fresh", "embed.alpha", "assistant.alpha",
             "portal.beta", "oidc.beta", "catalogue.beta", "crawl.beta.beta-web/site"]
    assert {k: rows[k]["result"] for k in green} == {k: "pass" for k in green}, rows
    red = {k for k, r in rows.items() if r["result"] == "fail"}
    # beta: the installation realm's metadata names realm alpha; its harvest source answers 503;
    # its last crawl is 100 h old; the embedder refused 7 passages. alpha: the App host is unknown.
    # gamma: its metadata names api.city.test as the resource.
    assert rows["prm.gamma"]["actual"] == "200 resource api.city.test http://idm.city.test/realms/jc"
    assert rows["prm.alpha"]["call"] == "GET portal.alpha.test/.well-known/oauth-protected-resource/api/v1/mcp"
    # The gateway's /api/mcp metadata is asked of the gateway host only, never of an Organization host.
    assert [h for m, h, p in edge[1] if p == "/.well-known/oauth-protected-resource/api/mcp"] == ["api.city.test"]
    assert {"prm.beta", "prm.gamma", "harvest.beta.beta-arcgis", "crawl.beta.beta-web/site.fresh", "embed.beta",
            "app.alpha.nosuch.apps.alpha.test"} <= red
    assert rows["embed.beta"]["actual"] == "7 refused, 0 waiting"
    assert rows["crawl.beta.beta-files/upload"]["result"] == "skip"
    assert rows["assistant.beta"]["result"] == "skip" and "--assistant beta=" in rows["assistant.beta"]["reason"]
    assert code == 1
    assert res["organizations"]["alpha"]["fail"] == 1 and res["organizations"]["beta"]["fail"] >= 4
    # No answer body reaches a row: statuses and counts only.
    assert not any("query_entities" in r["actual"] or "issuer" in r["actual"] for r in res["checks"])


def test_the_assistant_row_asks_about_an_own_page_and_fails_a_cached_or_direct_route(tmp_path, edge, public_sources):
    _, res = run(tmp_path, public_sources)
    row = {r["id"]: r for r in res["checks"]}["assistant.alpha"]
    asked = [m for k, h, m in edge[1] if k == "ASK"]
    assert len(asked) == 1 and "Parking permits" in asked[0] and res["questions"] == {"alpha": asked[0]}
    assert (row["result"], row["actual"]) == ("pass", "200, route search, answer, 1/1 own citations")
    # The next run reads this one's results and asks the other phrasing: no answer-cache key repeats.
    prev = tmp_path / "prev.json"
    prev.write_text(json.dumps(res))
    for done, route in (({"cached": True, "stagesMs": {"route": 2}}, "cached"),
                        ({"cached": False, "stagesMs": {"route": 4, "answer": 300}}, "direct")):
        edge[2].clear()
        edge[2].update(done)
        out = tmp_path / route
        out.mkdir()
        _, again = run(out, public_sources, "--previous", str(prev))
        row = {r["id"]: r for r in again["checks"]}["assistant.alpha"]
        assert row["result"] == "fail" and f"route {route}" in row["actual"] and row["reason"].startswith(route)
        assert again["questions"]["alpha"] != asked[0]


def test_an_index_without_a_titled_page_fails_the_assistant_row_by_name(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    doc["knowledge"][0]["titles"] = []
    _, res = run(tmp_path, public_sources, snap_doc=doc)
    row = {r["id"]: r for r in res["checks"]}["assistant.alpha"]
    assert row["result"] == "fail" and row["actual"] == "no title in 1 knowledge sources searched"
    assert "no public, embedded page or document with a title" in row["reason"]
    assert not [m for k, h, m in edge[1] if k == "ASK"]


def test_the_schedule_sets_the_cadence_and_a_sitemap_check_makes_it_daily():
    assert orgs.cron_period_hours("0 3 * * *") == 24
    assert orgs.cron_period_hours("0 3 * * 0") == orgs.cron_period_hours("0 3 * * 7") == 168
    assert orgs.cron_period_hours("0 0-23/6 * * *") == 6
    assert orgs.cron_period_hours("20 4 15 * *") >= 31 * 24
    assert orgs.cron_period_hours("0 3 1 * 1") <= 7 * 24  # day or weekday, as jc_core::cron reads it
    assert orgs.cron_period_hours("nightly") is None and orgs.cron_period_hours("0 3 31 2 *") is None
    weekly = {"source": "website", "schedule": "0 3 * * 0"}
    assert orgs.crawl_limit(None, 48) == (48, "no manifest names the source")
    assert orgs.crawl_limit(weekly, 48) == (192, "schedule 0 3 * * 0")
    assert orgs.crawl_limit({**weekly, "sitemap": True}, 48) == (48, "schedule 0 3 * * 0 with daily sitemap checks")
    assert orgs.crawl_limit({"source": "catalogue"}, 99) == (48, "schedule 0 3 * * *")
    assert orgs.crawl_limit({"schedule": "nightly"}, 48)[0] == 48


def test_a_sitemap_job_newer_than_the_last_full_crawl_is_the_newest_crawl():
    got, what = orgs.newest_crawl({"lastCrawl": at(65), "lastDone": {"at": at(11), "kind": "sitemap"}})
    assert round((NOW - got).total_seconds() / 3600) == 11 and what == "sitemap job"
    assert orgs.newest_crawl({"lastCrawl": at(2), "lastDone": {"at": at(30), "kind": "full"}})[1] == "sites.last_crawl"
    assert orgs.newest_crawl({"lastCrawl": None})[0] is None


def test_crawl_freshness_counts_sitemap_jobs_and_the_schedule_and_a_failed_job_is_red(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    alpha, beta = doc["knowledge"][0], doc["knowledge"][1]
    # alpha: weekly full crawl 65 h ago, the daily sitemap check 11 h ago, done (T-7397's zilina).
    alpha.update(lastCrawl=at(65), lastDone={"at": at(11), "kind": "sitemap"},
                 lastEnd={"state": "done", "at": at(11), "kind": "sitemap", "errorClass": None})
    # beta: its manifest crawls weekly with no sitemap, last crawled 100 h ago; the newest job failed.
    beta.update(lastDone={"at": at(100), "kind": "full"},
                lastEnd={"state": "failed", "at": at(5), "kind": "full", "errorClass": "the crawl failed"})
    (tmp_path / "manifests" / "beta").mkdir(parents=True)
    (tmp_path / "manifests" / "beta" / "beta-web-site.yaml").write_text(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: KnowledgeSource\nmetadata:\n  name: site\n"
        "  namespace: beta-web\nspec:\n  source: website\n  schedule: '0 3 * * 0'\n")
    (tmp_path / "manifests" / "beta" / "template.yaml").write_text("spec: {{ schedule }}\n")
    _, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {r["id"]: r for r in res["checks"]}
    fresh = rows["crawl.alpha.alpha-web/site.fresh"]
    assert fresh["result"] == "pass" and fresh["actual"] == "11.0 h (sitemap job)", fresh
    assert fresh["expected"] == "under 48 h (no manifest names the source)"
    fresh = rows["crawl.beta.beta-web/site.fresh"]
    assert fresh["result"] == "pass" and fresh["expected"] == "under 192 h (schedule 0 3 * * 0)", fresh
    assert rows["crawl.alpha.alpha-web/site.job"]["result"] == "pass"
    job = rows["crawl.beta.beta-web/site.job"]
    assert job["result"] == "fail" and job["actual"] == "full failed: the crawl failed", job
    assert rows["crawl.beta.beta-files/upload"]["result"] == "skip" and "crawl.beta.beta-files/upload.job" not in rows


def test_an_assistant_schema_before_migration_0025_skips_the_embed_row(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    for k in doc["knowledge"]:
        k["errors"] = None  # watch.assistant_sql(False): the column does not exist there
    _, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {r["id"]: r for r in res["checks"]}
    assert rows["embed.alpha"]["result"] == rows["embed.beta"]["result"] == "skip"
    assert "migration 0025" in rows["embed.beta"]["reason"]


def test_the_guard_refuses_writes_foreign_hosts_and_private_sources(tmp_path, edge, monkeypatch):
    port, seen, _ = edge
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


SLUG = """
mode = "slug"
base_host = "jc.city.test"

[mounts.portal]
path_prefix = ""

[[organizations]]
name = "alpha"
hosts = { portal = "portal.alpha.test", idm = "idm.alpha.test" }

[[organizations]]
name = "beta"
hosts = { portal = "portal.beta.test" }
"""


def test_a_slug_mode_config_probes_its_organizations_hosts_and_base_host_paths_whole(tmp_path):
    cfg = live.config("jc-dev", "jc.city.test", "127.0.0.1", SLUG, tmp_path)
    assert cfg["routes"] == [("", "jc.city.test", "")]
    assert cfg["hosts"] == ["idm.alpha.test", "jc.city.test", "portal.alpha.test", "portal.beta.test"]
    g = live.Guard(cfg)
    assert g.route(cfg["virtual"], "/login/realms/jc") == ("jc.city.test", "/login/realms/jc")
    # Without an idm host the sign-in realm is the installation's, under slug mode's /login.
    a = type("A", (), {"previous": None, "scheme": "https", "timeout": 5, "manifests": str(tmp_path)})()
    s = orgs.Suite(orgs.tomllib.loads(SLUG), {}, common.Results("orgs", "jc-dev"), a)
    assert (s.login, s.login_path) == ("jc.city.test", "/login")


def test_a_slug_mode_config_without_an_organization_host_is_refused(tmp_path):
    bare = SLUG.split("[[organizations]]")[0] + '[[organizations]]\nname = "auto"\nhost = "auto"\n'
    with pytest.raises(SystemExit, match="no host to probe"):
        live.config("jc-dev", "jc.city.test", "127.0.0.1", bare, tmp_path)
    with pytest.raises(SystemExit, match="neither a host-mode nor a slug-mode"):
        live.config("jc-dev", "jc.city.test", "127.0.0.1", SLUG.replace('"slug"', '"path"'), tmp_path)


class FakeBoard(watch.Board):
    def __init__(self):
        super().__init__(None, "jc-dev", "deployer")
        self.opened, self.updated, self.closed = [], [], []

    def open(self, key, rows, at):
        self.opened.append((key, [r["id"] for r in rows]))
        return f"T-{len(self.opened)}"

    def update(self, tid, rows, at):
        self.updated.append((tid, [r["id"] for r in rows]))

    def close(self, tid, key, why, at):
        self.closed.append((tid, key, why))


def results(**rows):
    orgs = {k.split(".")[1] for k in rows if k.split(".")[1] != "gateway"} | {"alpha", "beta"}
    return {"totals": {}, "organizations": {o: {} for o in orgs},
            "checks": [{"id": k, "target": k.split(".")[1], "result": v} for k, v in rows.items()]}


def test_three_red_checks_of_one_organization_are_one_incident_updated_and_closed_when_green():
    b = FakeBoard()
    red = {"app.alpha.x": "fail", "mcp.alpha": "fail", "oidc.alpha": "fail", "portal.alpha": "pass",
           "portal.beta": "pass"}
    state = watch.incidents({}, results(**red), None, b, "t1")
    assert b.opened == [("org/alpha", ["app.alpha.x", "mcp.alpha", "oidc.alpha"])]
    assert state == {"org/alpha": {"task": "T-1", "checks": ["app.alpha.x", "mcp.alpha", "oidc.alpha"]}}
    state = watch.incidents(state, results(**red), None, b, "t2")
    assert len(b.opened) == 1 and b.updated == []  # same red set: no new task, no rewrite
    state = watch.incidents(state, results(**{**red, "mcp.alpha": "pass"}), None, b, "t3")
    assert b.updated == [("T-1", ["app.alpha.x", "oidc.alpha"])] and state["org/alpha"]["task"] == "T-1"
    # An incomplete run closes nothing it did not see pass; it opens the cluster's snapshot incident.
    state = watch.incidents(state, results(**{"oidc.alpha": "pass"}), "snapshot: kubectl: denied", b, "t4")
    assert set(state) == {"org/alpha", "cluster"} and b.closed == []
    # A complete run that did not run alpha's held checks closes only the cluster's (T-7467).
    state = watch.incidents(state, results(**{"portal.alpha": "pass", "portal.beta": "pass"}), None, b, "t5")
    assert set(state) == {"org/alpha"} and [c[1] for c in b.closed] == ["cluster"]
    state = watch.incidents(state, results(**{"app.alpha.x": "pass", "oidc.alpha": "pass"}), None, b, "t6")
    assert state == {} and [c[1] for c in b.closed] == ["cluster", "org/alpha"]


def test_every_organizations_portal_red_is_one_incident_for_the_cluster():
    b = FakeBoard()
    rows = {"portal.alpha": "fail", "mcp.alpha": "fail", "portal.beta": "fail", "prm.gateway": "fail"}
    state = watch.incidents({}, results(**rows), None, b, "t1")
    assert b.opened == [("cluster", ["portal.alpha", "mcp.alpha", "portal.beta", "prm.gateway"])]
    assert list(state) == ["cluster"]
    # One Organization's portal back: the rest is that Organization's own incident again.
    state = watch.incidents(state, results(**{**rows, "portal.alpha": "pass", "mcp.alpha": "pass"}), None, b, "t2")
    assert b.opened[1] == ("org/beta", ["portal.beta"]) and set(state) == {"cluster", "org/beta"}
    assert b.updated == [("T-1", ["prm.gateway"])]


def test_the_per_check_incidents_of_t7309_close_on_the_next_green_run():
    b = FakeBoard()
    legacy = {"portal.alpha": "T-7323", "app.alpha.x": "T-7336", "prm.gateway": "T-7325", "crawl.beta.y": "T-7340"}
    green = {"portal.alpha": "pass", "app.alpha.x": "pass", "prm.gateway": "pass", "portal.beta": "pass"}
    state = watch.incidents(legacy, results(**green), None, b, "t1")
    # crawl.beta.y did not run: its incident stays open (T-7467), a person closes a check gone for good
    assert state == {"crawl.beta.y": "T-7340"} and b.opened == []
    assert sorted(b.closed) == [("T-7323", "portal.alpha", "passes"), ("T-7325", "prm.gateway", "passes"),
                                ("T-7336", "app.alpha.x", "passes")]
    # Still red on the first run of the grouped watch: listed in the Organization's incident, closed there.
    b = FakeBoard()
    state = watch.incidents({"mcp.alpha": "T-7327"}, results(**{"mcp.alpha": "fail", "portal.alpha": "pass"}),
                            None, b, "t1")
    assert b.closed == [("T-7327", "mcp.alpha", "is tracked in T-1")] and list(state) == ["org/alpha"]


def test_a_dry_placeholder_left_in_the_state_is_filed_as_a_real_incident():
    b = FakeBoard()
    stale = {"org/alpha": {"task": "dry-org/alpha", "checks": ["mcp.alpha"]}, "mcp.beta": "dry-mcp.beta"}
    state = watch.incidents(stale, results(**{"mcp.alpha": "fail", "portal.alpha": "pass"}), None, b, "t1")
    assert b.opened == [("org/alpha", ["mcp.alpha"])] and b.closed == []
    assert state == {"org/alpha": {"task": "T-1", "checks": ["mcp.alpha"]}}


def test_a_dry_run_reads_the_state_but_leaves_incidents_json_byte_identical(tmp_path, monkeypatch):
    state = tmp_path / "state"
    cfg = tmp_path / "watch.toml"
    cfg.write_text(f'''cluster = "jc-dev"\nkubeconfig = "k"\nnamespace = "jc"\npg_namespace = "pg"\n
edge = "127.0.0.1"\ndomain = "city.test"\nstate = "{state}"\nowner = "deployer"\nboard = "{tmp_path}"\n''')
    state.mkdir()
    before = b'{"portal.alpha": "T-7323"}'
    (state / "incidents.json").write_bytes(before)
    monkeypatch.setattr(watch, "snapshot", lambda cfg, run: (_ for _ in ()).throw(RuntimeError("503")))
    monkeypatch.setattr(watch.Board, "tool", lambda *a, **k: pytest.fail("the board was touched"))
    assert watch.main(["--config", str(cfg), "--dry-run"]) == 1  # the broken snapshot is red, nothing filed
    assert (state / "incidents.json").read_bytes() == before


def window(tmp_path, text, written):
    f = tmp_path / "maintenance-until"
    f.write_text(text)
    os.utime(f, (written.timestamp(), written.timestamp()))


def test_the_maintenance_window_holds_until_its_time_capped_at_sixty_minutes(tmp_path):
    now = dt.datetime(2026, 11, 19, 11, 30, tzinfo=dt.UTC)
    assert watch.maintenance_until(tmp_path, now) is None  # missing
    window(tmp_path, "2026-11-19T11:45:00Z", now - dt.timedelta(minutes=10))
    assert watch.maintenance_until(tmp_path, now) == dt.datetime(2026, 11, 19, 11, 45, tzinfo=dt.UTC)
    assert watch.maintenance_until(tmp_path, now + dt.timedelta(minutes=16)) is None  # after
    window(tmp_path, "2026-11-19T15:00:00Z", now - dt.timedelta(minutes=10))  # 3.5 h ahead: capped at +60 min
    assert watch.maintenance_until(tmp_path, now) == dt.datetime(2026, 11, 19, 12, 20, tzinfo=dt.UTC)
    assert watch.maintenance_until(tmp_path, now + dt.timedelta(minutes=51)) is None
    window(tmp_path, "soon", now)
    assert watch.maintenance_until(tmp_path, now) is None  # unreadable: normal filing


def test_a_run_inside_the_window_logs_maintenance_and_files_nothing(tmp_path, monkeypatch):
    state = tmp_path / "state"
    cfg = tmp_path / "watch.toml"
    cfg.write_text(f'''cluster = "jc-dev"\nkubeconfig = "k"\nnamespace = "jc"\npg_namespace = "pg"\n
edge = "127.0.0.1"\ndomain = "city.test"\nstate = "{state}"\nowner = "deployer"\nboard = "{tmp_path}"\n''')
    state.mkdir()
    (state / "incidents.json").write_text(json.dumps({"portal.alpha": "T-7323"}))
    window(state, (dt.datetime.now(dt.UTC) + dt.timedelta(minutes=20)).strftime("%Y-%m-%dT%H:%M:%SZ"),
           dt.datetime.now(dt.UTC))
    monkeypatch.setattr(watch, "snapshot", lambda cfg, run: (_ for _ in ()).throw(RuntimeError("503")))
    monkeypatch.setattr(watch.Board, "tool", lambda *a, **k: pytest.fail("the board was touched"))
    assert watch.main(["--config", str(cfg)]) == 0
    line = (state / "orgs.log").read_text().splitlines()[-1]
    assert "no result (snapshot: 503); maintenance until" in line and line.endswith("nothing filed")
    assert json.loads((state / "incidents.json").read_text()) == {"portal.alpha": "T-7323"}


def test_the_board_task_lists_the_red_checks_and_the_update_rewrites_only_that_list(tmp_path):
    b = board(tmp_path)
    rows = [{"id": "portal.bbsk", "actual": "503", "expected": "200", "call": "GET /"},
            {"id": "mcp.bbsk", "actual": "503", "expected": "401", "call": "POST /api/v1/mcp"}]
    assert b.open("org/bbsk", rows, "t1") == "T-9001"
    f = tmp_path / "T-9001-incident-jc-prod-orgs-bbsk.md"
    text = f.read_text()
    assert 'title: "Incident on jc-prod: Organization bbsk smoke red"' in text
    assert "incident: jc-prod/orgs/bbsk" in text and "- `mcp.bbsk`: 503" in text
    f.write_text(text + "\n- 2026-11-19T12:00Z **prod-deployer**: looking\n")
    b.update("T-9001", rows[:1], "t2")
    text = f.read_text()
    assert "Open checks as of t2 (1):" in text and "mcp.bbsk" not in text and "**prod-deployer**: looking" in text


# tasks/claim's rule (tasks/claim on s1): a claim is the dir .claims/<id>; a status changes only
# when .claims/<id>/owner names $AGENT_NAME; done moves the task to finished/.
CLAIM = r"""#!/bin/sh
cd "$(dirname "$0")"; id=$1; f=$(ls "$id"-*.md finished/"$id"-*.md 2>/dev/null | head -1)
if [ -z "$2" ]; then
  mkdir .claims/"$id" 2>/dev/null || { echo "$id already claimed" >&2; exit 1; }
  echo "$AGENT_NAME" > .claims/"$id"/owner; sed -i "s/^status:.*/status: doing/" "$f"; exit 0
fi
[ "$(cat .claims/"$id"/owner 2>/dev/null)" = "$AGENT_NAME" ] || { echo "$id is not yours" >&2; exit 1; }
sed -i "s/^status:.*/status: $2/" "$f"; [ "$2" = done ] && mkdir -p finished && mv "$f" finished/
exit 0
"""


def board(tmp_path):
    (tmp_path / ".claims").mkdir()
    for name, body in (("newid", "#!/bin/sh\necho T-9001\n"), ("note", "#!/bin/sh\nexit 0\n"), ("claim", CLAIM)):
        (tmp_path / name).write_text(body)
        (tmp_path / name).chmod(0o755)
    return watch.Board(tmp_path, "jc-prod", "prod-deployer")


def test_an_opened_incident_is_claimed_for_the_owner_and_closes_done(tmp_path):
    b = board(tmp_path)
    tid = b.open("org/bbsk", [{"id": "portal.bbsk", "actual": "503", "expected": "200", "call": "GET /"}], "t1")
    assert (tmp_path / ".claims" / tid / "owner").read_text().strip() == "prod-deployer"
    b.close(tid, "org/bbsk", "all checks pass", "t2")
    text = (tmp_path / "finished" / f"{tid}-incident-jc-prod-orgs-bbsk.md").read_text()
    assert "status: done" in text and "Closed t2 by orgs/watch.py: org/bbsk all checks pass." in text


def test_an_incident_opened_before_the_claim_is_claimed_then_closed(tmp_path):
    b = board(tmp_path)
    f = tmp_path / "T-7365-incident-jc-prod-orgs-crawl-smartcity-bbsk-web.md"
    f.write_text("---\nid: T-7365\nstatus: todo\nadmitted: owner 2026-11-19 T-7309\nowner: prod-deployer\nincident: jc-prod/orgs/crawl.smartcity.bbsk/web\n---\n")
    b.close("T-7365", "crawl.smartcity.bbsk/web", "passes", "t2")
    assert "status: done" in (tmp_path / "finished" / f.name).read_text()
    assert (tmp_path / ".claims" / "T-7365" / "owner").read_text().strip() == "prod-deployer"


def test_the_watch_never_closes_a_task_it_did_not_open(tmp_path):
    b = board(tmp_path)
    other = tmp_path / "T-7000-feature.md"
    other.write_text("---\nid: T-7000\nstatus: doing\nowner: worker-1\n---\n")
    foreign = tmp_path / "T-7001-incident-jc-dev-orgs-bbsk.md"
    foreign.write_text("---\nid: T-7001\nstatus: todo\nincident: jc-dev/orgs/bbsk\n---\n")
    for tid in ("T-7000", "T-7001"):
        with pytest.raises(RuntimeError, match="not this watch's incident"):
            b.close(tid, "org/bbsk", "all checks pass", "t2")
    assert not any((tmp_path / ".claims").iterdir())
    assert "status: doing" in other.read_text() and "status: todo" in foreign.read_text()


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
        if "exec" in args:
            # the server's statement limit ends before kubectl's (T-7506)
            assert f"SET statement_timeout = '{watch.QUERY_SECONDS}s'" in args and timeout > watch.QUERY_SECONDS
        if args[-1] == watch.EMBED_ERROR_SQL:
            return "SET\n" + json.dumps({"embedError": embed_error})
        if args[-1] == watch.PIPELINES_SQL:
            return "SET\n" + json.dumps({"pipelines": []})
        if args[-1].startswith("SELECT"):
            return "SET\n" + json.dumps({"organizations": []} if "organizations" in args[-1] else {"knowledge": []})
        if "logs" in args:
            return LOG
        return "pod/postgres-cluster-1\n" if "pod" in args else 'mode = "host"\n'

    monkeypatch.setattr(watch, "kubectl", fake)
    cfg = {"namespace": "jc", "jc_configmap": "jc", "pg_namespace": "pg", "pg_cluster": "postgres-cluster"}
    toml, snap = watch.snapshot(cfg, tmp_path)
    # jc.toml comes from the ConfigMap: the jc image has no shell tools to exec
    assert calls[0] == ("-n", "jc", "get", "configmap", "jc", "-o", r"jsonpath={.data.jc\.toml}")
    assert not any("exec" in c and c[1] == "jc" for c in calls)
    assert toml.read_text() == 'mode = "host"\n'
    assert json.loads(snap.read_text()) == {"organizations": [], "pipelines": [], "knowledge": [], **watch.log_facts(LOG),
                                            "poolUnavailable": {"error": "watch.toml names no prometheus_namespace"},
                                            "refusedCounter": {"window": "30m", "error": "watch.toml names no prometheus_namespace"}}
    # the log of every jc pod's jc-server container, the last 30 min only
    assert ("-n", "jc", "logs", "deploy/jc", "-c", "jc-server", "--since=30m", "--all-pods=true",
            "--prefix=false") in calls
    queries = [c for c in calls if c[-1].startswith(("SELECT", "WITH"))]
    assert len(queries) == 4 and all("SET default_transaction_read_only = on" in c for c in queries)
    # Before assistant migration 0025 the query names no chunks.embed_error (the column does not exist).
    assert ("embed_error" in queries[-1][-1]) is embed_error
    for sql in (watch.PORTAL_SQL, watch.PIPELINES_SQL, watch.assistant_sql(True), watch.assistant_sql(False), watch.EMBED_ERROR_SQL):
        assert not any(w in sql.upper() for w in ("INSERT", "UPDATE ", "DELETE", "DROP", "TRUNCATE", "C.TEXT"))
    # Crawl jobs give their time, kind and the error's class only: never the error's own text.
    assert "split_part(coalesce(j.error, ''), ':', 1)" in watch.assistant_sql(True)
    assert "j.error" not in watch.assistant_sql(True).replace("split_part(coalesce(j.error, ''), ':', 1)", "")


def runs(*rows) -> list[dict]:
    """Newest first, as the snapshot lists them: (hours ago, delivered, failed)."""
    return [{"at": at(h), "delivered": d, "failed": f} for h, d, f in rows]


def test_a_pipeline_that_delivered_and_now_only_fails_is_red_with_its_newest_error():
    hourly_403 = {"project": "p", "pipeline": "air", "lastDelivered": at(29),
                  "runs": runs((0.2, 0, 40), (1.2, 0, 40), (2.2, 0, 40), (3.2, 0, 40)),
                  "error": "write refused: 403 Open Data Is Read-Only\u0007"}
    result, got = orgs.delivering(hourly_403, NOW)
    assert result == "fail", got
    assert got.startswith("0 delivered, 80 failed in 2 runs of the last 2 h; last delivered 29.0 h ago")
    assert got.endswith("newest error: write refused: 403 Open Data Is Read-Only")
    # it delivers again: green on the next run
    back = {**hourly_403, "lastDelivered": at(0.1), "runs": runs((0.1, 12, 0), *[(h, 0, 40) for h in (1.2, 2.2)])}
    assert orgs.delivering(back, NOW)[0] == "pass"


def test_never_delivered_paused_and_an_empty_source_are_not_silent_pipelines():
    never = {"lastDelivered": None, "runs": runs((0.5, 0, 9), (1.5, 0, 9))}
    assert orgs.delivering(never, NOW) == ("skip", "never delivered in 7 days")
    paused = {"lastDelivered": at(30), "runs": runs((30, 5, 0), (31, 5, 0))}
    assert orgs.delivering(paused, NOW)[0] == "skip"
    assert "paused" in orgs.delivering(paused, NOW)[1]
    empty = {"lastDelivered": at(6), "runs": runs((0.5, 0, 0), (1.5, 0, 0), (2.5, 0, 0), (6, 3, 0))}
    quiet = "0 delivered, 0 failed in 2 runs of the last 2 h; last delivered 6.0 h ago (the source sent nothing)"
    assert orgs.delivering(empty, NOW) == ("pass", quiet)


def test_a_daily_pipeline_gets_two_periods_before_it_counts_as_silent():
    daily = {"lastDelivered": at(23), "runs": runs((0.5, 0, 3), (23, 8, 0), (47, 8, 0), (71, 8, 0))}
    assert orgs.delivering(daily, NOW) == ("pass", "last delivered 23.0 h ago, 3 runs in the last 48 h")
    two_days = {"lastDelivered": at(49), "runs": runs((0.5, 0, 3), (24.5, 0, 3), (49, 8, 0), (73, 8, 0))}
    assert orgs.delivering(two_days, NOW)[0] == "fail"


def test_the_delivering_rows_belong_to_the_projects_organization(tmp_path, edge, public_sources):
    doc = snapshot(public_sources)
    doc["pipelinesAt"] = at(0)
    doc["pipelines"] = [
        {"project": "beta-web", "pipeline": "events", "lastDelivered": at(20), "runs": runs((0.5, 0, 4), (1.5, 0, 4)),
         "error": "403 Open Data Is Read-Only"},
        {"project": "alpha-web", "pipeline": "air", "lastDelivered": at(0.5), "runs": runs((0.5, 7, 0), (1.5, 7, 0))},
        {"project": "unmapped", "pipeline": "x", "lastDelivered": at(20), "runs": runs((0.5, 0, 4))},
    ]
    _, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {r["id"]: r for r in res["checks"] if ".delivering." in r["id"]}
    assert set(rows) == {"pipelines.delivering.beta-web/events", "pipelines.delivering.alpha-web/air"}
    red = rows["pipelines.delivering.beta-web/events"]
    assert (red["target"], red["result"]) == ("beta", "fail") and "403 Open Data Is Read-Only" in red["actual"]
    assert rows["pipelines.delivering.alpha-web/air"]["result"] == "pass"


# jc-server lines as tracing's text format writes them (T-7458): two refusals of one Pipeline's
# account, one of the project account, a person's (account "-", not counted), a quality refusal
# (no account), and two drift rounds, the newest of which wins.
LOG = """\x1b[2m2026-11-20T17:01:00Z\x1b[0m  INFO request{module="gateway"}: context_gateway::handlers::ngsi_ld: write refused: an open-data Endpoint slug=pbyq operation=createEntity account=pl-equipaments account_project=barcelona
2026-11-20T17:01:30Z  INFO request{module="gateway"}: context_gateway::handlers::ngsi_ld: write refused: an open-data Endpoint slug=pbyq operation=upsertEntities account=pl-equipaments account_project=barcelona
2026-11-20T17:02:00Z  INFO request{module="gateway"}: context_gateway::handlers::ngsi_ld: write refused: no Policy grants it slug=dhiw operation=createEntity account=pipelines account_project=zilina
2026-11-20T17:02:10Z  INFO request{module="gateway"}: context_gateway::handlers::ngsi_ld: write refused: no Policy grants it slug=dhiw operation=createEntity account=- account_project=-
2026-11-20T17:02:20Z  INFO request{module="gateway"}: context_gateway::quality: write refused: it breaks a quality rule space=air rule=r1
2026-11-20T17:00:00Z  INFO joinedcontext_portal::host_registry: every project with Pipelines has an Organization
2026-11-20T17:05:00Z  WARN joinedcontext_portal::host_registry: projects with Pipelines and no Organization: their runs keep no history and their imports cannot be checked; an operator adds each Organization on the Organizations page projects=barcelona:19,zilina:3
"""


def test_the_log_window_counts_each_pipelines_refused_writes_and_the_newest_unplaced_report():
    facts = watch.log_facts(LOG)
    assert facts["refusedWrites"] == [
        {"project": "barcelona", "account": "pl-equipaments", "reason": "an open-data Endpoint", "count": 2},
        {"project": "zilina", "account": "pipelines", "reason": "no Policy grants it", "count": 1},
    ]
    assert facts["unplacedReport"] == [{"project": "barcelona", "pipelines": 19}, {"project": "zilina", "pipelines": 3}]
    assert facts["unattributedRefusals"] == []
    quiet = watch.log_facts(LOG.splitlines()[5] + "\n")
    assert quiet == {"logWindow": "30m", "unplacedReport": [], "refusedWrites": [], "unattributedRefusals": [],
                     "logMinutes": quiet["logMinutes"]}
    assert watch.log_facts("")["unplacedReport"] is None


def test_a_log_that_cannot_be_read_is_named_never_an_empty_window(monkeypatch):
    def refuse(cfg, *args, timeout=60):
        raise RuntimeError("kubectl logs deploy/jc: forbidden")

    monkeypatch.setattr(watch, "kubectl", refuse)
    got = watch.jc_log({"namespace": "jc"})
    assert got == {"logWindow": "30m", "logError": "kubectl logs deploy/jc: forbidden"}
    assert orgs.refused_writes(got)[0] == orgs.unplaced_projects(got)[0] == "fail"


def test_refused_pipeline_writes_fail_the_row_naming_project_account_and_count():
    result, got = orgs.refused_writes(watch.log_facts(LOG))
    assert result == "fail"
    assert got == ("3 refused in the jc-server log of the last 30m: barcelona/pl-equipaments 2 (an open-data Endpoint); "
                   "zilina/pipelines 1 (no Policy grants it)")
    # no refusal in the log and no counter read: the log never passes the row (T-7467)
    result, got = orgs.refused_writes(watch.log_facts(LOG.splitlines()[5]))
    assert result == "skip" and "never passes this row" in got and "no gateway counter" in got
    assert orgs.refused_writes({})[0] == "skip"


def test_a_project_with_pipelines_and_no_organization_fails_the_row_by_name():
    result, got = orgs.unplaced_projects(watch.log_facts(LOG))
    assert (result, got) == ("fail", "2 without an Organization: barcelona (19 Pipelines), zilina (3 Pipelines)")
    assert orgs.unplaced_projects(watch.log_facts(LOG.splitlines()[5])) == (
        "pass", "every project with Pipelines has an Organization")
    assert orgs.unplaced_projects(watch.log_facts(""))[0] == "skip"
    assert orgs.unplaced_projects({})[0] == "skip"


def test_the_installation_rows_belong_to_the_deployment_and_fail_the_run(tmp_path, edge, public_sources):
    doc = {**snapshot(public_sources), **watch.log_facts(LOG)}
    code, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {r["id"]: r for r in res["checks"] if r["id"] in ("pipelines.refused", "pipelines.unplaced")}
    assert {(i, r["target"], r["result"]) for i, r in rows.items()} == {
        ("pipelines.refused", "t", "fail"), ("pipelines.unplaced", "t", "fail")}
    assert "barcelona/pl-equipaments 2" in rows["pipelines.refused"]["actual"]
    assert "barcelona (19 Pipelines)" in rows["pipelines.unplaced"]["actual"]
    assert code == 1
    # no token, body or record in the rows: names and counts only
    assert not any(w in json.dumps(rows) for w in ("slug=", "Bearer", "operation="))


def test_a_refusal_of_a_jc_before_t7458_names_no_account_and_still_fails_by_slug():
    old = ("2026-11-20T17:03:00Z  INFO request{module=\"gateway\"}: context_gateway::handlers::ngsi_ld: "
           "write refused: an open-data Endpoint slug=pbyqbuyoeikryocvqb5uev7nxbx5mr4a operation=createEntity\n")
    facts = watch.log_facts(old * 3)
    assert facts["refusedWrites"] == []
    assert facts["unattributedRefusals"] == [
        {"slug": "pbyqbuyoeikryocvqb5uev7nxbx5mr4a", "reason": "an open-data Endpoint", "count": 3}]
    assert orgs.refused_writes(facts) == (
        "fail", "3 refused in the jc-server log of the last 30m: ?/slug pbyqbuyoeikryocvqb5uev7nxbx5mr4a 3 (an open-data Endpoint)")


# The lines jc writes on every cluster (log_format = "json"): the refusal exactly as the gateway's
# test a_refused_write_names_the_workload_and_its_project writes it in .json() mode, the Portal's
# report in the same shape (T-7458 bounce: the text regexes alone passed on 0 there).
LOG_JSON = """{"timestamp":"2026-11-20T17:42:22.471166Z","level":"INFO","fields":{"message":"write refused: an open-data Endpoint","slug":"pbyqbuyoeikryocvqb5uev7nxbx5mr4a","operation":"createEntity","account":"pl-equipaments","account_project":"barcelona"},"target":"context_gateway::handlers::ngsi_ld"}
{"timestamp":"2026-11-20T17:42:23Z","level":"INFO","fields":{"message":"write refused: an open-data Endpoint","slug":"pbyqbuyoeikryocvqb5uev7nxbx5mr4a","operation":"createEntity"},"target":"context_gateway::handlers::ngsi_ld"}
{"timestamp":"2026-11-20T17:40:00Z","level":"INFO","fields":{"message":"every project with Pipelines has an Organization"},"target":"joinedcontext_portal::host_registry"}
{"timestamp":"2026-11-20T17:45:00Z","level":"WARN","fields":{"message":"projects with Pipelines and no Organization: their runs keep no history and their imports cannot be checked; an operator adds each Organization on the Organizations page","projects":"barcelona:19,zilina:3"},"target":"joinedcontext_portal::host_registry"}
not json {"
"""


def test_the_json_lines_jc_writes_on_the_clusters_fail_both_rows():
    facts = watch.log_facts(LOG_JSON)
    assert facts["refusedWrites"] == [
        {"project": "barcelona", "account": "pl-equipaments", "reason": "an open-data Endpoint", "count": 1}]
    assert facts["unattributedRefusals"] == [
        {"slug": "pbyqbuyoeikryocvqb5uev7nxbx5mr4a", "reason": "an open-data Endpoint", "count": 1}]
    assert facts["unplacedReport"] == [{"project": "barcelona", "pipelines": 19}, {"project": "zilina", "pipelines": 3}]
    assert orgs.refused_writes(facts)[0] == orgs.unplaced_projects(facts)[0] == "fail"
    # the newest report wins in JSON too: a later "none" line passes the row
    placed = LOG_JSON.splitlines()[3] + "\n" + LOG_JSON.splitlines()[2]
    assert orgs.unplaced_projects(watch.log_facts(placed))[0] == "pass"


def test_lines_with_kubectls_pod_prefix_count_the_same():
    # kubectl logs --all-pods=true prefixes each line with [pod/<pod>/<container>] unless
    # --prefix=false: on jc-prod 154 barcelona refusals counted 0 that way (T-7458 bounce 18:11Z)
    prefixed = "\n".join(f"[pod/jc-7d9f8c6b5-x2k4q/jc-server] {line}" for line in LOG_JSON.splitlines())
    assert watch.log_facts(prefixed) == watch.log_facts(LOG_JSON)
    text = "[pod/jc-1/jc-server] INFO write refused: an open-data Endpoint slug=abc operation=createEntity"
    assert watch.log_facts(text)["unattributedRefusals"] == [
        {"slug": "abc", "reason": "an open-data Endpoint", "count": 1}]


def test_a_log_window_truncated_by_rotation_never_passes_the_refused_row():
    # kubelet rotated jc-server's file at 18:14:38Z: --since=30m at 18:18Z read 4 min, 0 refusals,
    # and the row passed (jc-prod 2026-11-20T18:18Z, T-7458)
    now = dt.datetime(2026, 11, 20, 18, 18, 17, tzinfo=dt.UTC)
    quiet = ('[pod/jc-1/jc-server] {"timestamp":"2026-11-20T18:14:38.120Z","level":"INFO","fields":{"message":"x"}}\n'
             '{"timestamp":"2026-11-20T18:18:00Z","level":"INFO","fields":{"message":"y"}}\n')
    facts = watch.log_facts(quiet, now)
    assert facts["logMinutes"] == 3
    result, got = orgs.refused_writes(facts)
    assert result == "skip" and got.startswith("0 refused in the jc-server log (3 min of 30m read)")
    # refusals inside a short window still fail: a red is never hidden
    refused = watch.log_facts(LOG_JSON.splitlines()[0].replace("17:42:22", "18:15:00"), now)
    assert orgs.refused_writes(refused)[0] == "fail"
    # even the full window never passes: only the gateway counter does (T-7467); text lines carry
    # their time at the start
    full = watch.log_facts("2026-11-20T17:49:00Z  INFO jc: started\n" + quiet, now)
    assert full["logMinutes"] == 29 and orgs.refused_writes(full)[0] == "skip"
    # no line with a time: the window is unknown, never green
    assert watch.log_facts("", now)["logMinutes"] is None
    assert orgs.refused_writes(watch.log_facts("", now))[0] == "skip"
    # a time inside a message does not stretch the window
    assert watch.log_facts("INFO y at 2026-11-20T10:00:00Z\n", now)["logMinutes"] is None


# T-7467: jc-prod's T-7465 closed at 20:00:03Z on a run whose pipelines.refused skipped (log rotated)
# while barcelona was still refused.
def test_an_incident_whose_check_skips_stays_open_and_closes_only_on_pass():
    b = FakeBoard()
    rows = lambda v: {**results(**{"portal.alpha": "pass"}),
                      "checks": [{"id": "pipelines.refused", "target": "jc-prod", "result": v},
                                 {"id": "portal.alpha", "target": "alpha", "result": "pass"}]}
    state = watch.incidents({}, rows("fail"), None, b, "t1")
    assert b.opened == [("cluster", ["pipelines.refused"])]
    state = watch.incidents(state, rows("skip"), None, b, "t2")
    assert b.closed == [] and state["cluster"]["checks"] == ["pipelines.refused"]
    # missing row (not probed this run) keeps it open as well
    state = watch.incidents(state, results(**{"portal.alpha": "pass"}), None, b, "t3")
    assert b.closed == [] and list(state) == ["cluster"]
    state = watch.incidents(state, rows("pass"), None, b, "t4")
    assert state == {} and b.closed == [("T-1", "cluster", "all checks pass")]


def test_a_held_check_that_turns_skip_stays_listed_while_another_is_red():
    b = FakeBoard()
    state = watch.incidents({}, results(**{"mcp.alpha": "fail", "oidc.alpha": "fail"}), None, b, "t1")
    state = watch.incidents(state, results(**{"mcp.alpha": "skip", "oidc.alpha": "fail"}), None, b, "t2")
    # the list is not rewritten: mcp.alpha skipped, it did not pass
    assert b.updated == [] and state["org/alpha"]["checks"] == ["mcp.alpha", "oidc.alpha"]
    state = watch.incidents(state, results(**{"mcp.alpha": "pass", "oidc.alpha": "fail"}), None, b, "t3")
    assert b.updated == [("T-1", ["oidc.alpha"])] and b.closed == []
    block = watch.checks_block([{"id": "mcp.alpha", "result": "skip", "actual": "rotated"}], "t")
    assert "(skip, not yet green) rotated" in block


# T-7494: jc-prod recorded no run from 02:23Z to 08:00Z with its pool empty, and no row was red.
def test_an_exhausted_pool_turns_its_row_red_and_only_a_read_gauge_passes():
    row = lambda got: orgs.pool_available({"poolUnavailable": got})
    assert row({"seconds": 20260})[0] == "fail" and "20260 s" in row({"seconds": 20260})[1]
    assert row({"seconds": 120})[0] == "fail"
    assert row({"seconds": 119})[0] == row({"seconds": 0})[0] == "pass"
    assert row({"error": "forbidden"})[0] == orgs.pool_available({})[0] == "skip"


def test_no_run_recorded_for_30_min_turns_its_row_red():
    row = lambda last: orgs.runs_recorded({"pipelinesAt": "2026-11-22T08:03:00Z", "lastRecorded": last})
    assert row("2026-11-22T02:23:18Z")[0] == "fail"
    assert row("2026-11-22T07:33:00Z")[0] == "fail"
    assert row("2026-11-22T07:34:00Z")[0] == "pass"
    assert row(None)[0] == row("2026-11-01T00:00:00Z")[0] == orgs.runs_recorded({})[0] == "skip"


def test_the_pool_gauge_is_read_from_prometheus_and_an_unread_one_says_why(monkeypatch):
    asked = []
    answer = ['[{"metric": {}, "value": [1, "240"]}]']

    def kubectl(cfg, *args, timeout=60):
        if "get" in args:
            return "pod/prometheus-0\n"
        asked.append(args[-1])
        return answer[0]

    monkeypatch.setattr(watch, "kubectl", kubectl)
    cfg = {"namespace": "jc-prod", "prometheus_namespace": "prometheus"}
    assert watch.pool_gauge(cfg) == {"poolUnavailable": {"seconds": 240}}
    assert asked == ['max(jc_portal_db_unavailable_seconds{namespace="jc-prod"})']
    answer[0] = "[]"
    assert "no jc_portal_db_unavailable_seconds" in watch.pool_gauge(cfg)["poolUnavailable"]["error"]
    assert "error" in watch.pool_gauge({"namespace": "jc-prod"})["poolUnavailable"]


def test_the_exhausted_pool_rows_belong_to_the_deployment_and_fail_the_run(tmp_path, edge, public_sources):
    doc = {**snapshot(public_sources), "poolUnavailable": {"seconds": 20260},
           "pipelinesAt": "2026-11-22T08:03:00Z", "lastRecorded": "2026-11-22T02:23:18Z"}
    code, res = run(tmp_path, public_sources, snap_doc=doc)
    rows = {(r["id"], r["target"], r["result"]) for r in res["checks"]}
    assert {("database.pool", "t", "fail"), ("pipelines.recorded", "t", "fail")} <= rows
    assert code == 1


COUNTER = {"refusedCounter": {"window": "30m", "rows": [
    {"project": "barcelona", "account": "pl-equipaments", "reason": "open-data", "count": 186},
    {"project": "bratislava", "account": "pl-mhd", "reason": "open-data", "count": 4}]}}


def test_the_counter_decides_the_refused_row_and_the_log_can_only_fail_it():
    # barcelona still unplaced, the log rotated to 3 min with nothing in it: red from the counter
    now = dt.datetime(2026, 11, 20, 18, 18, 17, tzinfo=dt.UTC)
    rotated = watch.log_facts('{"timestamp":"2026-11-20T18:15:00Z","level":"INFO","fields":{"message":"x"}}\n', now)
    assert orgs.refused_writes({**rotated, **COUNTER}) == (
        "fail", ("190 refused in the last 30m (gateway counter): barcelona/pl-equipaments 186 (open-data); "
                 "bratislava/pl-mhd 4 (open-data)"))
    zero = {"refusedCounter": {"window": "30m", "rows": []}}
    assert orgs.refused_writes({**rotated, **zero}) == (
        "pass", "0 refused writes of a Pipeline in the last 30m (gateway counter)")
    # the counter at 0 and refusals in the log: the log still fails it
    result, got = orgs.refused_writes({**watch.log_facts(LOG), **zero})
    assert result == "fail" and "the counter shows 0" in got
    # a log that could not be read does not matter while the counter answers
    assert orgs.refused_writes({"logWindow": "30m", "logError": "forbidden", **zero})[0] == "pass"
    # the counter unread: skip with its reason, or fail when the log names refusals
    unread = {"refusedCounter": {"window": "30m", "error": "jc's admin listener in jc was not scraped"}}
    result, got = orgs.refused_writes({**rotated, **unread})
    assert result == "skip" and "not scraped" in got
    assert orgs.refused_writes({**watch.log_facts(LOG), **unread})[0] == "fail"


def test_the_counter_is_read_with_promtool_inside_the_prometheus_pod(monkeypatch):
    calls = []
    answers = {"SCRAPED": '[{"metric":{},"value":[1795206856.2,"1"]}]',
               "REFUSED": '[{"metric":{"account":"pl-equipaments","account_project":"barcelona","reason":"open-data"},'
                          '"value":[1795206856.2,"186.00000000000003"]},'
                          '{"metric":{"account":"pl-x","account_project":"zilina","reason":"no-policy"},"value":[1,"0"]}]'}

    def fake(cfg, *args, timeout=60):
        calls.append(args)
        if "get" in args:
            return "pod/prometheus-prometheus-0\n"
        return answers["SCRAPED" if args[-1].startswith("min(") else "REFUSED"]

    monkeypatch.setattr(watch, "kubectl", fake)
    cfg = {"namespace": "jc-prod", "prometheus_namespace": "prometheus"}
    assert watch.refused_counter(cfg) == {"refusedCounter": {"window": "30m", "rows": [
        {"project": "barcelona", "account": "pl-equipaments", "reason": "open-data", "count": 186}]}}
    assert calls[0] == ("-n", "prometheus", "get", "pod", "-o", "name", "-l", "app.kubernetes.io/name=prometheus")
    expr = calls[2][-1]
    assert calls[2][:12] == ("-n", "prometheus", "exec", "pod/prometheus-prometheus-0", "-c", "prometheus", "--",
                             "promtool", "query", "instant", "-o", "json")
    assert 'jc_gateway_writes_refused_total{namespace="jc-prod"}' in expr and "increase(" in expr and "offset 30m" in expr
    # not scraped through the window, or Prometheus not reachable: an error, never an empty list
    answers["SCRAPED"] = '[{"metric":{},"value":[1,"0"]}]'
    assert "not scraped" in watch.refused_counter(cfg)["refusedCounter"]["error"]
    answers["SCRAPED"] = "[]"
    assert "rows" not in watch.refused_counter(cfg)["refusedCounter"]

    def refuse(cfg, *args, timeout=60):
        raise RuntimeError("kubectl exec prometheus-prometheus-0: forbidden")

    monkeypatch.setattr(watch, "kubectl", refuse)
    assert watch.refused_counter(cfg) == {"refusedCounter": {"window": "30m",
                                                             "error": "kubectl exec prometheus-prometheus-0: forbidden"}}
    assert watch.refused_counter({"namespace": "jc"})["refusedCounter"]["error"].startswith("watch.toml names no")

