"""The public status page on status.{domain} (T-5102): states from the probe's results, the 90-day
history, the deployer's incident notes, the rendered page in sk and en, what it never shows, and
the rendered workload, route and policies."""

import datetime as dt
import importlib.util
import json
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).parent.parent
spec = importlib.util.spec_from_file_location("status", ROOT / "components/probes/files/status.py")
status = importlib.util.module_from_spec(spec)
spec.loader.exec_module(status)

NOW = dt.datetime(2026, 10, 21, 20, 30, tzinfo=dt.UTC).timestamp()
A, B = "4ysiqy53qik5qyvawwaj7wgylpt4uoxa", "bbsk7events2026"


def row(surface, ok, endpoint=None, target="dev", at=NOW - 30, reason=None):
    return {"target": target, "endpoint": endpoint, "surface": surface, "ok": ok,
            "reason": reason if reason is not None else (None if ok else "status 503"), "seconds": 0.2, "at": at}


def healthy():
    rows = [row(s, True) for s in ("catalogue", "portal", "assistant")]
    rows += [row(s, True, e) for s in ("ngsi-ld", "odata", "ogc", "sta", "websocket", "mcp", "csv", "ical") for e in (A, B)]
    return rows


class Clock:
    def __init__(self, now):
        self.now = now

    def __call__(self):
        return self.now


def test_states_up_degraded_down_and_unknown():
    rows = healthy()
    rows = [r for r in rows if r["surface"] not in ("odata", "ogc", "ical")]
    rows += [row("odata", True, A), row("odata", False, B), row("ogc", False, A), row("ogc", False, B)]
    # Another target's rows and stale rows never count.
    rows += [row("ical", True, A, target="smartcity"), row("ical", True, A, at=NOW - status.STALE_SECONDS - 1)]
    states = status.current(rows, "dev", NOW)
    assert states["catalogue"]["state"] == "up"
    assert states["odata"] == {"state": "degraded", "ok": 1, "total": 2, "failing": [B]}
    assert states["ogc"]["state"] == "down"
    assert states["ical"]["state"] == "unknown"
    assert status.overall(states) == "some_down"
    assert status.overall(status.current(healthy(), "dev", NOW)) == "all_up"
    assert status.overall(status.current([], "dev", NOW)) == "no_data"
    assert status.overall(status.current("not a list", "dev", NOW)) == "no_data"


def test_history_counts_minutes_keeps_90_days_and_survives_a_damaged_file():
    history = {"odata": {"2026-01-01": [5, 0, 0]}}
    up = status.current(healthy(), "dev", NOW)
    for _ in range(3):
        status.add_minute(history, up, NOW)
    down = status.current([row("odata", False, A)], "dev", NOW)
    status.add_minute(history, down, NOW)
    assert history["odata"] == {"2026-10-21": [3, 0, 1]}, "a day older than 90 is dropped"
    assert "ical" not in status.add_minute({}, down, NOW), "an unknown minute counts nowhere"
    assert status.total_uptime(history, "odata") == 75.0
    assert status.total_uptime(history, "sta") == 100.0
    assert status.total_uptime({}, "sta") is None
    days = status.days_of(history, "odata", NOW)
    assert len(days) == 90 and days[-1] == ("2026-10-21", [3, 0, 1]) and days[0][0] == "2026-07-24"
    assert status.load_history("{broken") == {}
    assert status.load_history(json.dumps({"odata": {"2026-10-20": [1, 2, 3], "x": [1, 1, 1], "2026-10-19": [-1, 0, 0]},
                                           "evil": {"2026-10-20": [1, 0, 0]}})) == {"odata": {"2026-10-20": [1, 2, 3]}}


def test_uptime_never_rounds_an_outage_up_to_100():
    assert status.pct(status.uptime([129599, 0, 1])) == "99.99"
    assert status.pct(100.0) == "100"
    assert status._day_class([1440, 0, 0]) == "up"
    assert status._day_class([1430, 0, 10]) == "degraded"
    assert status._day_class([1000, 0, 440]) == "down"
    assert status._day_class([0, 0, 0]) == "none"


INCIDENTS = {"incidents": [
    {"id": "2026-10-21-odata-timeouts", "environments": ["dev"], "start": "2026-10-21T20:15:00Z",
     "surfaces": ["odata", "ogc"], "title": {"sk": "Pomalé OData", "en": "Slow OData"},
     "note": {"sk": "Hľadáme príčinu <script>x</script>.", "en": "We are looking for the cause <script>x</script>."}},
    {"id": "2026-10-01-catalogue", "environments": ["dev", "smartcity"], "start": "2026-10-01T08:00:00Z",
     "end": "2026-10-01T09:30:00Z", "surfaces": ["catalogue"], "title": {"sk": "Katalóg nedostupný", "en": "Catalogue down"},
     "note": {"sk": "Obnovené po aktualizácii.", "en": "Restored after the upgrade."}},
    {"id": "2026-10-02-prod-only", "environments": ["smartcity"], "start": "2026-10-02T08:00:00Z", "end": "2026-10-02T09:00:00Z",
     "surfaces": ["mcp"], "title": {"sk": "MCP", "en": "MCP"}, "note": {"sk": "x", "en": "x"}},
    {"id": "2026-05-01-old", "environments": ["dev"], "start": "2026-05-01T08:00:00Z", "end": "2026-05-01T09:00:00Z",
     "surfaces": ["mcp"], "title": {"sk": "Staré", "en": "Old"}, "note": {"sk": "x", "en": "x"}},
]}


def test_incidents_load_for_their_environment_and_split_ongoing_from_past():
    incidents = status.load_incidents(json.dumps(INCIDENTS), "dev")
    assert [i["id"] for i in incidents] == ["2026-10-21-odata-timeouts", "2026-10-01-catalogue", "2026-05-01-old"]
    ongoing, past = status.split_incidents(incidents, NOW)
    assert [i["id"] for i in ongoing] == ["2026-10-21-odata-timeouts"]
    assert [i["id"] for i in past] == ["2026-10-01-catalogue"], "older than 90 days drops off the page"
    assert status.load_incidents("", "dev") == []
    assert status.load_incidents(json.dumps({"incidents": None}), "dev") == []


@pytest.mark.parametrize("edit,field", [
    ({"id": "Has Spaces"}, "id"),
    ({"start": "2026-10-21 20:15"}, "start"),
    ({"start": "2026-10-21T20:15:00"}, "start"),
    ({"end": "2026-10-20T00:00:00Z"}, "end"),
    ({"surfaces": ["odata", "kafka"]}, "surfaces"),
    ({"surfaces": []}, "surfaces"),
    ({"environments": "dev"}, "environments"),
    ({"title": {"sk": "len slovensky"}}, "title"),
    ({"note": {"sk": "", "en": "x"}}, "note"),
    ({"severity": "high"}, "keys"),
])
def test_a_malformed_incident_names_the_field_to_fix(edit, field):
    item = {**INCIDENTS["incidents"][0], **edit}
    with pytest.raises(ValueError, match=field):
        status.load_incidents(json.dumps({"incidents": [item]}), "dev")
    with pytest.raises(ValueError, match="id"):
        status.load_incidents(json.dumps({"incidents": [INCIDENTS["incidents"][0]] * 2}), "dev")


def test_the_committed_incident_file_is_valid_for_every_environment():
    doc = yaml.safe_load((ROOT / "components/probes/files/incidents.yaml").read_text())
    for env in ("dev", "smartcity"):
        status.load_incidents(json.dumps(doc), env)


def page(lang="sk", rows=None, history=None, incidents=None):
    states = status.current(healthy() if rows is None else rows, "dev", NOW)
    return status.render(lang, "dev.joinedcontext.com", states, history or {},
                         status.load_incidents(json.dumps(incidents or {"incidents": []}), "dev"), NOW)


def test_render_shows_every_surface_with_its_state_and_90_days_in_both_languages():
    history = {"odata": {"2026-10-21": [1439, 0, 1], "2026-10-20": [1440, 0, 0]}}
    sk, en = page("sk", history=history), page("en", history=history)
    assert '<html lang="sk">' in sk and '<html lang="en">' in en
    assert "Všetko je v prevádzke" in sk and "All systems operational" in en
    for names in status.SURFACES.values():
        assert f"<h3>{names['sk']}</h3>" in sk and f"<h3>{names['en']}</h3>" in en
    assert sk.count('class="bar ') == 90 * len(status.SURFACES)
    # The label names the days measured (two here), never 90 before 90 days exist (T-5184).
    assert "99.96 % uptime over 2 days" in en and "uptime over 90 days" not in en
    assert "99.96 % dostupnosť za 2 dni" in sk
    assert "Zatiaľ bez meraní" in sk and "No measurements yet" in en
    assert 'href="/en/"' in sk and 'href="/sk/"' in en
    assert '<meta http-equiv="refresh" content="60">' in sk


def test_render_names_failing_endpoints_and_never_a_reason_host_or_address():
    secret_reason = "status 500 from 10.43.0.17 context-gateway.jc-dev.svc.cluster.local <html>"
    rows = [r for r in healthy() if r["surface"] != "ngsi-ld"]
    rows += [row("ngsi-ld", True, A), row("ngsi-ld", False, B, reason=secret_reason)]
    rows += [row("portal", False, reason=secret_reason)]
    html = page("en", rows=[r for r in rows if not (r["surface"] == "portal" and r["ok"])], incidents=INCIDENTS)
    assert "Some interfaces are down" in html
    assert "1 of 2 sources answer correctly" in html and f"Not answering: {B}" in html
    assert "10.43." not in html and "svc.cluster.local" not in html and "status 500" not in html
    assert "<script>" not in html and "&lt;script&gt;" in html, "incident notes are escaped"
    assert "Slow OData" in html and "since 2026-10-21 20:15 UTC" in html
    assert "Catalogue down" in html and "2026-10-01 08:00 – 2026-10-01 09:30 UTC" in html
    assert "MCP</h3><p class=\"when\">" not in html, "another environment's incident stays off"
    assert "Old" not in html
    doc = status.machine(status.current(rows, "dev", NOW), {}, status.load_incidents(json.dumps(INCIDENTS), "dev"), NOW)
    assert "10.43." not in json.dumps(doc) and "reason" not in json.dumps(doc)
    assert doc["generatedAt"] == "2026-10-21T20:30:00Z" and len(doc["surfaces"]) == len(status.SURFACES)


def test_render_without_probe_data_says_so_and_offers_an_empty_incident_list():
    html = page("sk", rows=[])
    assert "Sonda zatiaľ neposlala výsledky" in html
    assert html.count("Bez údajov") == len(status.SURFACES)
    assert "Za posledných 90 dní žiadne hlásené udalosti." in html


def test_language_negotiation_prefers_slovak_unless_english_ranks_higher():
    assert status.prefer(None) == "sk"
    assert status.prefer("en-GB,en;q=0.9") == "en"
    assert status.prefer("cs,sk;q=0.8,en;q=0.7") == "sk"
    assert status.prefer("en;q=0.5,sk;q=0.9") == "sk"
    assert status.prefer("de,fr") == "sk"
    assert status.prefer("en;q=bad") == "sk"


def test_an_injected_outage_shows_on_the_next_minute_and_recovers(tmp_path):
    """Outage injection without a cluster: the probe's next results turn the page red at the next
    tick (one minute), the history keeps the minute, a recovery turns it green again."""
    clock, answers = Clock(NOW), {"results": healthy()}
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: answers)
    assert pg.answer("/en/", None)[2].decode().count("All systems operational") == 1
    clock.now += 60
    answers = {"results": [r if r["surface"] != "mcp" else {**r, "ok": False, "at": clock.now} for r in healthy()]}
    pg.get = lambda: answers
    pg.tick()
    body = pg.answer("/en/", None)[2].decode()
    assert "Some interfaces are down" in body
    assert json.loads(pg.answer("/status.json", None)[2])["status"] == "some_down"
    stored = json.loads((tmp_path / "history.json").read_text())
    assert stored["mcp"]["2026-10-21"] == [0, 0, 1] and stored["odata"]["2026-10-21"] == [1, 0, 0]
    clock.now += 60
    pg.get = lambda: {"results": [{**r, "at": clock.now} for r in healthy()]}
    pg.tick()
    assert "All systems operational" in pg.answer("/en/", None)[2].decode()
    # A restart reads the history back.
    again = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                        get=lambda: {"results": []})
    assert again.history["mcp"]["2026-10-21"] == [1, 0, 1]


def test_an_unreachable_probe_is_unknown_never_up(tmp_path):
    def refused():
        raise OSError("connection refused")

    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=Clock(NOW),
                     get=refused)
    pg.tick()
    assert "No data" in pg.answer("/en/", None)[2].decode()
    assert not (tmp_path / "history.json").exists() or json.loads((tmp_path / "history.json").read_text()) == {}


def test_routes_negotiate_cache_and_refuse_the_rest(tmp_path):
    clock = Clock(NOW)
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: {"results": healthy()})
    code, kind, body, extra = pg.answer("/", "en-US,en;q=0.9")
    assert code == 200 and kind.startswith("text/html") and b'lang="en"' in body
    assert extra["Vary"] == "Accept-Language" and "max-age=30" in extra["Cache-Control"]
    assert "stale-if-error" in extra["Cache-Control"]
    assert b'lang="sk"' in pg.answer("/?utm=x", None)[2]
    assert pg.answer("/en", None)[:2] == (308, "text/plain") and pg.answer("/en", None)[3] == {"Location": "/en/"}
    assert pg.answer("/status.css", None)[1].startswith("text/css")
    assert pg.answer("/../etc/passwd", None)[0] == 404
    assert pg.answer("/history.json", None)[0] == 404, "the volume is never served"
    assert pg.answer("/healthz", None)[0] == 200
    clock.now += 181
    assert pg.answer("/healthz", None)[0] == 503
    assert "default-src 'none'" in status.CSP and "frame-ancestors 'none'" in status.CSP


def test_uptime_days_count_from_the_first_sample_and_cap_at_90():
    history = {"sta": {"2026-10-22": [0, 0, 0], "2026-10-19": [10, 0, 0], "2026-10-21": [5, 0, 5]}}
    assert status.measured_days(history, "sta", NOW) == 3, "from 10-19 to today 10-21; an empty future day counts nothing"
    assert status.measured_days(history, "odata", NOW) == 0
    assert status.measured_days({"sta": {"2026-01-01": [1, 0, 0]}}, "sta", NOW) == 90
    assert status.measured_days({"sta": {"2026-10-21": [1, 0, 0]}}, "sta", NOW) == 1
    assert [status.days_text("sk", n) for n in (1, 2, 4, 5, 90)] == ["1 deň", "2 dni", "4 dni", "5 dní", "90 dní"]
    assert [status.days_text("en", n) for n in (1, 2)] == ["1 day", "2 days"]
    assert status.total_uptime(history, "sta") == pytest.approx(75.0), "days before the first sample count nowhere"
    doc = status.machine(status.current(healthy(), "dev", NOW), history, [], NOW)
    sta = next(c for c in doc["surfaces"] if c["id"] == "sta")
    assert sta["uptimeDays"] == 3 and sta["uptime"] == pytest.approx(75.0) and "uptime90" not in sta


def test_page_and_status_json_agree_on_every_surface_state_including_no_data():
    rows = [r for r in healthy() if r["surface"] not in ("sta", "ical")]
    rows += [row("sta", True, A, at=NOW - status.STALE_SECONDS - 1), row("ical", False, A)]
    states = status.current(rows, "dev", NOW)
    doc = status.machine(states, {}, [], NOW)
    for lang in ("sk", "en"):
        html = status.render(lang, "dev.joinedcontext.com", states, {}, [], NOW)
        shown = dict(re.findall(r'<h3>([^<]+)</h3><span class="state ([a-z]+)"', html))
        assert {s: shown[names[lang]] for s, names in status.SURFACES.items()} == {c["id"]: c["state"] for c in doc["surfaces"]}
    assert {c["id"]: c["state"] for c in doc["surfaces"]}["sta"] == "unknown"


def minute(clock, pg, down=(), degraded=(), unknown=()):
    """One probe round and one tick: `down` surfaces fail everywhere, `degraded` on one Endpoint."""
    clock.now += 60
    rows = []
    for r in healthy():
        if r["surface"] in unknown:
            continue
        ok = r["ok"] and r["surface"] not in down and not (r["surface"] in degraded and r["endpoint"] == B)
        rows.append({**r, "ok": ok, "at": clock.now})
    pg.get = lambda: {"results": rows}
    pg.tick()
    return json.loads(pg.answer("/status.json", None)[2])


def test_an_outage_of_three_minutes_opens_an_incident_and_ten_green_minutes_close_it(tmp_path):
    clock = Clock(NOW)
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: {"results": healthy()})
    # Two bad minutes, a green one, two bad again: no run of three, no incident.
    for down in (["mcp"], ["mcp"], [], ["mcp"], ["mcp"]):
        assert minute(clock, pg, down=down)["incidents"] == []
    first_bad = clock.now - 60
    doc = minute(clock, pg, down=["mcp"], degraded=["odata"])
    assert [i["id"] for i in doc["incidents"]] == [f"auto-{dt.datetime.fromtimestamp(first_bad, dt.UTC):%Y%m%d-%H%M}"]
    incident = doc["incidents"][0]
    assert incident["automatic"] is True and incident["end"] is None and incident["surfaces"] == ["mcp"]
    assert incident["title"] == {"sk": "Rozhrania mimo prevádzky", "en": "Interfaces down"}
    assert incident["start"] == dt.datetime.fromtimestamp(first_bad, dt.UTC).isoformat().replace("+00:00", "Z")
    en = pg.answer("/en/", None)[2].decode()
    assert "Ongoing incidents" in en and "Interfaces down" in en and "opened this entry automatically" in en
    assert "Prebiehajúce udalosti" in pg.answer("/sk/", None)[2].decode()
    # odata joins once its own run reaches three minutes; an unknown minute (probe silent) changes nothing.
    minute(clock, pg, down=["mcp"], degraded=["odata"])
    minute(clock, pg, unknown=list(status.SURFACES))
    doc = minute(clock, pg, degraded=["odata"])
    assert doc["incidents"][0]["surfaces"] == ["odata", "mcp"] and doc["incidents"][0]["end"] is None
    last_bad = clock.now
    # A restart keeps the open incident; nine green minutes keep it open, the tenth closes it.
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: {"results": []})
    for _ in range(9):
        assert minute(clock, pg)["incidents"][0]["end"] is None
    doc = minute(clock, pg)
    closed = doc["incidents"][0]
    assert closed["end"] == dt.datetime.fromtimestamp(last_bad + 60, dt.UTC).isoformat().replace("+00:00", "Z")
    assert closed["note"]["en"].endswith("closed this entry automatically.")
    en = pg.answer("/en/", None)[2].decode()
    assert "Ongoing incidents" not in en and "Incidents in the last 90 days" in en and "Interfaces down" in en
    assert "No incidents reported" not in en
    # A closed incident leaves the page after 90 days.
    clock.now += 91 * 86400
    assert minute(clock, pg)["incidents"] == []


def test_a_degraded_only_outage_is_titled_degraded_and_a_damaged_watch_file_starts_fresh(tmp_path):
    clock = Clock(NOW)
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: {"results": healthy()})
    for _ in range(3):
        doc = minute(clock, pg, degraded=["csv"])
    assert doc["incidents"][0]["title"]["sk"] == "Rozhrania čiastočne obmedzené"
    (tmp_path / "watch.json").write_text('{"runs": {"csv": [3, 0]}, "open": {"id": "../x", "surfaces": ["csv"]}, "closed": []}')
    again = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                        get=lambda: {"results": []})
    assert again.watch == status.new_watch()
    for text in ("", "[]", "{not json", '{"runs": 1, "open": null, "closed": []}'):
        assert status.load_watch(text) == status.new_watch()


def test_a_hand_note_over_the_same_surfaces_and_time_replaces_the_automatic_text():
    watch = status.new_watch()
    states = status.current(healthy(), "dev", NOW)
    bad = {**states, "odata": {**states["odata"], "state": "down"}}
    for i in range(3):
        status.watch_minute(watch, bad, NOW + 60 * i)
    notes = status.load_incidents(json.dumps(INCIDENTS), "dev")
    shown = status.incidents_shown(notes, watch)
    assert not any(i["automatic"] for i in shown), "the deployer's open odata note covers it"
    other = status.load_incidents(json.dumps({"incidents": [{**INCIDENTS["incidents"][0], "surfaces": ["ogc"]}]}), "dev")
    assert [i["automatic"] for i in status.incidents_shown(other, watch)] == [True, False], "another surface: both stand"
    earlier = status.load_incidents(json.dumps({"incidents": [{**INCIDENTS["incidents"][0], "start": "2026-10-20T01:00:00Z",
                                                               "end": "2026-10-20T02:00:00Z"}]}), "dev")
    assert any(i["automatic"] for i in status.incidents_shown(earlier, watch)), "no overlap: the automatic text stays"


def test_the_automatic_text_holds_no_host_address_or_reason(tmp_path):
    secret = "status 500 from 10.43.0.17 context-gateway.jc-dev.svc.cluster.local"
    clock = Clock(NOW)
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=clock,
                     get=lambda: {"results": []})
    for _ in range(4):
        clock.now += 60
        rows = [{**r, "ok": False, "reason": secret, "at": clock.now} for r in healthy()]
        pg.get = lambda rows=rows: {"results": rows}
        pg.tick()
    texts = [pg.answer(p, None)[2].decode() for p in ("/sk/", "/en/", "/status.json")] + [(tmp_path / "watch.json").read_text()]
    assert all("10.43." not in t and "svc.cluster" not in t and "status 500" not in t for t in texts)
    flat = json.dumps(status.AUTO_TEXT)
    assert not re.search(r"https?:|\.svc|\d+\.\d+\.\d+\.\d+|joinedcontext\.com", flat)


# ---- rendered


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def test_rendered_status_page_is_anonymous_pinned_and_reads_the_probe(dev):
    pod = one(dev, "Deployment", "status-page")["spec"]["template"]["spec"]
    container = pod["containers"][0]
    assert container["image"].startswith("docker.io/library/python:3.13.17-slim-trixie@sha256:")
    env = {e["name"]: e.get("value") for e in container["env"]}
    assert env["STATUS_TARGET"] == "dev" and env["STATUS_DOMAIN"] == "dev.joinedcontext.com"
    assert re.fullmatch(r"http://probe\.[a-z0-9-]+\.svc\.cluster\.local:9100/results\.json", env["STATUS_PROBE_URL"])
    status.load_incidents(env["STATUS_INCIDENTS"], "dev")
    assert not any("valueFrom" in e for e in container["env"]) and not container.get("envFrom")
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert pod["securityContext"]["runAsNonRoot"] is True
    assert pod.get("automountServiceAccountToken") is not True
    one(dev, "PersistentVolumeClaim", "status-page-files")
    config = one(dev, "ConfigMap", "status-page")
    assert config["data"]["status.py"] == (ROOT / "components/probes/files/status.py").read_text()


def test_rendered_status_page_policies_admit_the_edge_and_reach_only_the_probe(dev):
    page_policy = one(dev, "NetworkPolicy", "status-page")["spec"]
    assert [p["port"] for r in page_policy["ingress"] for p in r["ports"]] == [8080]
    ports = {(p["protocol"], p["port"]) for rule in page_policy["egress"] for p in rule["ports"]}
    assert ports == {("TCP", 9100), ("UDP", 53), ("TCP", 53)}
    assert not any("ipBlock" in t for r in page_policy["egress"] for t in r["to"])
    probe_policy = one(dev, "NetworkPolicy", "probe")["spec"]
    peers = [t["podSelector"]["matchLabels"] for r in probe_policy["ingress"] for t in r["from"] if "podSelector" in t]
    assert {"app.kubernetes.io/name": "probes-status"} in peers


# ---- backup and restore drill age (T-5289)


def test_backups_are_fresh_until_26_hours_and_the_drill_until_35_days_and_only_when_passed():
    fresh = status.backup_cells({"backupAt": NOW - 25 * 3600, "drillAt": NOW - 34 * 86400, "drillResult": "passed"}, NOW)
    assert fresh["backup"]["state"] == "fresh" and fresh["drill"]["state"] == "fresh"
    old = status.backup_cells({"backupAt": NOW - 27 * 3600, "drillAt": NOW - 36 * 86400, "drillResult": "passed"}, NOW)
    assert old["backup"]["state"] == "stale" and old["drill"]["state"] == "stale"
    failed = status.backup_cells({"backupAt": NOW - 3600, "drillAt": NOW - 86400, "drillResult": "failed"}, NOW)
    assert failed["drill"]["state"] == "stale"
    never = status.backup_cells({"backupAt": None, "drillAt": None, "drillResult": None}, NOW)
    assert never["backup"] == {"at": None, "state": "stale", "result": None}
    assert status.backup_cells(None, NOW) is None


def test_the_page_and_status_json_show_backup_and_drill_age_from_the_probe(tmp_path):
    answers = {"results": healthy(), "backups": {"backupAt": NOW - 5 * 3600, "drillAt": NOW - 12 * 86400,
                                                 "drillResult": "passed"}}
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=Clock(NOW),
                     get=lambda: answers)
    en = pg.answer("/en/", None)[2].decode()
    assert "Database backups" in en and "5 h ago" in en and "12 days ago" in en
    assert "restore and row counts passed" in en
    sk = pg.answer("/sk/", None)[2].decode()
    assert "Zálohy databázy" in sk and "pred 12 dňami" in sk
    doc = json.loads(pg.answer("/status.json", None)[2])
    assert doc["backups"]["backup"] == {"at": "2026-10-21T15:30:00Z", "state": "fresh"}
    assert doc["backups"]["drill"]["result"] == "passed"
    # Backups never change the interfaces' banner.
    assert doc["status"] == "all_up"


def test_a_probe_without_backups_shows_no_backup_section(tmp_path):
    pg = status.Page("dev", "dev.joinedcontext.com", "http://probe.test/results.json", [], str(tmp_path), clock=Clock(NOW),
                     get=lambda: {"results": healthy()})
    assert "Database backups" not in pg.answer("/en/", None)[2].decode()
    assert json.loads(pg.answer("/status.json", None)[2])["backups"] is None
