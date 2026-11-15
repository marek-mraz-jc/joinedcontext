"""scripts/smoke-catalogue.py against recorded answers (T-4716): every verdict of Testing/05 §4.1,
the owner's limits (pace, budget, deadline, Retry-After) and the rotation of the slow checks."""

import csv
import importlib.util
import json
import urllib.parse
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/smoke-catalogue.py"
spec = importlib.util.spec_from_file_location("smoke_catalogue", SCRIPT)
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

DOMAIN = "smartcity.example.org"
API = f"https://api.{DOMAIN}/api/endpoint"
CKAN = f"https://opendata.{DOMAIN}"
JSON, GEO, CSV = "application/json", "application/geo+json", "text/csv; charset=utf-8"


def answer(status=200, body=b"", ctype=JSON, **kw):
    if not isinstance(body, bytes):
        body = json.dumps(body).encode()
    return smoke.Answer(status, ctype, body, **kw)


def package(name, slug, typ, ogc=True, ws=True, datastore="ds-" , host=None, region="SK032", org=""):
    base = f"https://{host or 'api.' + DOMAIN}/api/endpoint/{slug}/"
    resources = [
        {"format": "NGSI-LD", "url": base + "ngsi-ld/v1/entities", "name": f"NGSI-LD API ({typ})", "id": f"ngsi-{name}"},
        {"format": "OData", "url": base + f"odata/v4/{typ}"},
        {"format": "MCP", "url": base + "mcp"},
        {"format": "CSV", "url": base + "file.csv"},
        {"format": "JSON", "url": base + "schema/index.json"},
    ]
    if ogc:
        resources.append({"format": "OGCFEAT", "url": base + f"ogc/features/collections/{typ}/items"})
    if ws:
        resources.append({"format": "WebSocket", "url": "wss" + base[5:] + "ws"})
    if datastore:
        resources.append({"format": "CSV", "url": f"{CKAN}/datastore/dump/{datastore}{name}",
                          "datastore_active": True, "id": f"{datastore}{name}"})
    extras = [{"key": "spatial_uri", "value": f"http://data.europa.eu/nuts/code/{region}"}] if region else []
    return {"name": name, "resources": resources, "extras": extras, "organization": {"name": org}}


def row(typ, n=1, located=True, **attrs):
    """One clean NGSI-LD row, normalized; Banská Bystrica when located."""
    entity = {"id": f"urn:ngsi-ld:{typ}:{n}", "type": typ, "name": {"type": "Property", "value": f"{typ} {n}"}}
    if located:
        entity["location"] = {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.15, 48.74]}}
    return {**entity, **attrs}


def model(typ, *attrs, required=()):
    return {"definitions": {typ: {"properties": {a: {} for a in ("name", "location", *attrs)},
                                  **({"required": list(required)} if required else {})}}}


def count_query(data):
    return {**smoke.sample_query(data, 1), "type": data.type, "count": "true"}


FILTERS = f"{CKAN}/api/3/action/package_search?" + urllib.parse.urlencode(
    {"rows": 0, "facet.field": json.dumps(["vocab_jc_format", "vocab_jc_topic"])})


def world(packages, public, rows=10, datastore_total=10, overrides=None):
    """The answers of a healthy installation for these datasets, with `overrides` by URL."""
    answers = {
        FILTERS: answer(body={"result": {"count": len(packages), "search_facets": {
            "vocab_jc_format": {"items": [{"name": "NGSI-LD", "count": len(packages)}]},
            "vocab_jc_topic": {"items": [{"name": "ENVI", "count": 1}]}}}}),
        f"{CKAN}/api/3/action/package_search?rows=1000&start=0":
            answer(body={"result": {"count": len(packages), "results": packages}}),
        f"https://api.{DOMAIN}/catalog.jsonld":
            answer(body={"dcat:dataset": [{"@id": f"{API}/{s}"} for s in public]}),
    }
    for p in packages:
        data = smoke.describe(p, DOMAIN)
        base, t = f"{API}/{data.slug}/", data.type
        answers.update({
            base + "ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data)):
                answer(body=[row(t, 1, "OGCFEAT" in data.formats), row(t, 2, "OGCFEAT" in data.formats)],
                       ctype="application/ld+json"),
            base + "mcp": answer(body={"result": {"tools": [{"name": "query"}]}}),
            base + f"odata/v4/{t}?" + urllib.parse.urlencode(smoke.odata_query(data)): answer(body={"@odata.count": rows, "value": [{}]}),
            base + f"ogc/features/collections/{t}/items?limit=1": answer(body={"features": [{}]}, ctype=GEO),
            base + "schema/index.json": answer(body={"models": [{"types": [t], "version": 1}]}),
            base + "schema/v1/model.schema.json": answer(body=model(t)),
            base + "ngsi-ld/v1/types": answer(body={"typeList": [t]}),
            base + "file.csv?" + urllib.parse.urlencode(smoke.sample_query(data, 1)): answer(body=b"id,type\nurn:1,T\n", ctype=CSV),
            base + "ngsi-ld/v1/entities?" + urllib.parse.urlencode(count_query(data)):
                answer(body=[row(t)], ctype="application/ld+json", results=str(rows)),
        })
        if "OGCFEAT" in data.formats:  # CKAN's /map/ is a 404 without one (jc_map_page)
            answers[f"{CKAN}/map/{p['name']}"] = answer(body=b"<div data-jc-map='{&#34;layers&#34;: [{}]}'>",
                                                        ctype="text/html")
        for ds in data.datastore:
            answers[f"{CKAN}/api/3/action/datastore_search?resource_id={ds}&limit=0"] = answer(
                body={"result": {"total": datastore_total}})
            answers[f"{CKAN}/api/3/action/resource_view_list?id={ds}"] = answer(
                body={"result": [{"view_type": "datatables_view"}]})
    answers.update(overrides or {})
    return answers


class Fake:
    """send, ws, clock and sleep of a Client, recording every request and every pause."""

    def __init__(self, answers, sockets=None):
        self.answers, self.sockets = answers, sockets or {}
        self.now, self.sent, self.pauses = 0.0, [], []

    def send(self, method, url, headers, body, limit):
        self.sent.append((method, url))
        got = self.answers.get(url, answer(404, {"type": "resource-not-found"}))
        got = got.pop(0) if isinstance(got, list) else got
        # send() reads at most `limit` bytes: a longer body arrives cut, as it does live.
        return smoke.Answer(**{**got.__dict__, "body": got.body[:limit]})

    def ws(self, url, typ):
        self.sent.append(("WS", url))
        return self.sockets.get(url, (101, 201, ""))

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.pauses.append(seconds)
        self.now += seconds

    def client(self, **kw):
        return smoke.Client(self.send, self.ws, self.clock, self.sleep, **kw)


def table(answers, sockets=None, tmp_path=None, now="20261019T060000Z", **kw):
    fake = Fake(answers, sockets)
    client = fake.client(**kw)
    previous, at = smoke.previous_table(tmp_path, "prod") if tmp_path else ({}, "")
    datasets, public = smoke.inventory(client, DOMAIN)
    # The count pass reads through a Fake of its own: `fake.sent` stays the budgeted client's.
    counts = smoke.Counts(DOMAIN, Fake(answers).send).cells(datasets)
    rows = smoke.Run(client, DOMAIN, previous, at, now).table(datasets, public, counts)
    return {r["dataset"] + ("" if r["dataset"] != "(no dataset)" else " " + r["endpoint"]): r for r in rows}, fake


def test_a_complete_dataset_is_ok_on_every_interface():
    rows, _ = table(world([package("air", "a1", "AirQualityObserved")], ["a1"]))
    row = rows["air"]
    assert {c: smoke.verdict(row[c]) for c in smoke.COLUMNS} == {c: "ok" for c in smoke.COLUMNS}
    assert row["type"] == "AirQualityObserved"


def test_a_missing_model_is_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    rows, _ = table(world([p], ["a1"], overrides={f"{API}/a1/schema/index.json": answer(404)}))
    assert rows["air"]["model"].startswith("ko: schema/index.json answered 404")


def test_a_model_that_lacks_the_type_is_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    rows, _ = table(world([p], ["a1"], overrides={
        f"{API}/a1/schema/index.json": answer(body={"models": [{"types": ["Other"]}]})}))
    assert rows["air"]["model"] == "ko: no model lists AirQualityObserved"


def test_a_dataset_without_a_datastore_table_is_a_warning_not_a_ko():
    rows, _ = table(world([package("air", "a1", "AirQualityObserved", datastore="")], ["a1"]))
    assert rows["air"]["datastore"] == "warn: no datastore table"


def test_a_datastore_ten_percent_off_is_a_warning_and_within_one_percent_is_ok():
    off, _ = table(world([package("air", "a1", "AirQualityObserved")], ["a1"], rows=100, datastore_total=90))
    assert off["air"]["datastore"] == "warn: 90 rows, the platform holds 100 (OData @odata.count)"
    near, _ = table(world([package("air", "a1", "AirQualityObserved")], ["a1"], rows=100, datastore_total=101))
    assert near["air"]["datastore"] == "ok: 101 rows = 100"


def test_a_shared_type_is_compared_by_its_own_is_part_of_or_not_at_all():
    a, b = package("one", "s1", "Person"), package("two", "s1", "Person")
    rows, _ = table(world([a, b], ["s1"], rows=50, datastore_total=7))
    assert rows["one"]["datastore"] == "ok: 7 rows; not compared, Person is shared by 2 datasets"
    c = package("three", "s2", "Road")
    c["resources"] = [dict(r, url=r["url"] + "?isPartOf=https%3A%2F%2Fx%2Fdataset%2Fthree")
                      if r["format"] == "OGCFEAT" else r for r in c["resources"]]
    url = f"{API}/s2/ogc/features/collections/Road/items?limit=1&isPartOf=https%3A%2F%2Fx%2Fdataset%2Fthree"
    rows, _ = table(world([c], ["s2"], rows=500, datastore_total=7,
                          overrides={url: answer(body={"features": [{}], "numberMatched": 7}, ctype=GEO)}))
    assert rows["three"]["datastore"] == "ok: 7 rows = 7"


def test_a_dev_host_in_the_production_catalogue_is_a_ko():
    p = package("bb", "d1", "CouncilMeeting", host="api.dev.example.org")
    rows, _ = table(world([p], ["d1"]))
    assert rows["bb"]["inventory"] == "ko: API on another installation's host api.dev.example.org"


def test_a_public_endpoint_without_a_dataset_and_a_dataset_without_an_endpoint_are_ko():
    lonely = {"name": "lonely", "resources": [{"format": "CSV", "url": "https://example.org/x.csv"}]}
    rows, _ = table(world([package("air", "a1", "AirQualityObserved"), lonely], ["a1", "z9"]))
    assert rows["(no dataset) z9"]["inventory"] == "ko: a public Endpoint that no dataset names"
    assert rows["lonely"]["inventory"] == "ko: the dataset lists no Endpoint distribution"
    assert rows["air"]["inventory"] == "ok"


def test_an_endpoint_missing_from_the_feed_is_a_ko():
    rows, _ = table(world([package("air", "a1", "AirQualityObserved")], []))
    assert rows["air"]["inventory"] == "ko: its Endpoint is not in the public DCAT feed"


def test_a_websocket_not_served_is_ok_only_with_the_documented_404():
    p = package("air", "a1", "AirQualityObserved", ws=False)
    ws = f"wss://api.{DOMAIN}/api/endpoint/a1/ws"
    refused, _ = table(world([p], ["a1"]), {ws: (404, 0, "")})
    assert refused["air"]["websocket"] == "ok: not served, refused 404 as API/02 names"
    wrong, _ = table(world([p], ["a1"]), {ws: (405, 0, "")})
    assert wrong["air"]["websocket"] == "ko: not served, but the upgrade answered 405"
    served, _ = table(world([package("air", "a1", "AirQualityObserved")], ["a1"]), {ws: (101, 403, "")})
    assert served["air"]["websocket"] == "ko: upgrade 101, subscribe 403"


def test_a_type_without_location_skips_ogc_and_has_no_map_page():
    rows, fake = table(world([package("budget", "b1", "Budget", ogc=False)], ["b1"]))
    assert rows["budget"]["ogc"].startswith("skip: the dataset lists no OGC distribution")
    assert rows["budget"]["map"] == "ok: no location, no map page"
    assert not any("/ogc/" in url for _, url in fake.sent)
    shown, _ = table(world([package("budget", "b1", "Budget", ogc=False)], ["b1"], overrides={
        f"{CKAN}/map/budget": answer(body=b"<div data-jc-map='{}'>", ctype="text/html")}))
    assert shown["budget"]["map"] == "ko: no location, but the map page answered 200 text/html"


def test_a_timeout_is_a_ko_with_its_time():
    p = package("air", "a1", "AirQualityObserved")
    url = f"{API}/a1/odata/v4/AirQualityObserved?%24top=1&%24count=true&%24select=id"
    rows, _ = table(world([p], ["a1"], overrides={url: answer(0, error="timeout after 20 s")}))
    assert rows["air"]["odata"] == "ko: AirQualityObserved answered timeout after 20 s, no @odata.count"


def test_an_mcp_event_stream_answer_counts_its_tools():
    stream = b'event: message\ndata: {"result": {"tools": [{"name": "a"}, {"name": "b"}]}}\n\n'
    assert smoke.mcp_tools(answer(body=stream, ctype="text/event-stream")) == 2
    assert smoke.mcp_tools(answer(body=b"<html>", ctype="text/html")) == 0


def test_requests_are_paced_counted_and_asked_once():
    a, b = package("one", "s1", "Person"), package("two", "s1", "Person")
    rows, fake = table(world([a, b], ["s1"]))
    urls = [url for _, url in fake.sent]
    assert len(urls) == len(set(urls)), "a shared answer is fetched once"
    assert all(p >= smoke.INTERVAL - 1e-9 for p in fake.pauses)
    assert smoke.verdict(rows["two"]["mcp"]) == "ok"


def test_a_spent_budget_leaves_cells_not_this_run_and_never_ko():
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(5)]
    rows, fake = table(world(packages, [f"e{i}" for i in range(5)]), budget=12)
    assert len([s for s in fake.sent]) <= 12
    cells = [r[c] for r in rows.values() for c in smoke.COLUMNS]
    assert "not this run (never)" in cells
    assert not any(smoke.verdict(c) == "ko" for c in cells)


def test_the_deadline_stops_the_run_at_ten_minutes():
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(200)]
    fake = Fake(world(packages, [f"e{i}" for i in range(200)]))
    client = fake.client()
    datasets, public = smoke.inventory(client, DOMAIN)
    smoke.Run(client, DOMAIN, {}, "", "now").table(datasets, public)
    assert client.count <= smoke.BUDGET and fake.now <= smoke.DEADLINE


def test_a_429_waits_its_retry_after_and_the_retry_is_counted():
    fake = Fake({"https://x/": [answer(429, retry_after="7"), answer(200, {"ok": True})]})
    client = fake.client()
    got = client.fetch("https://x/")
    assert got.status == 200 and client.count == 2 and 7 in fake.pauses


def test_the_slow_checks_rotate_least_recently_checked_first(tmp_path):
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(3)]
    previous = tmp_path / "smoke-catalogue-prod-20261019T050000Z.tsv"
    with previous.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, ["dataset", "endpoint", "type", *smoke.COLUMNS], delimiter="\t")
        writer.writeheader()
        writer.writerow({"dataset": "d0", **{c: "ok" for c in smoke.COLUMNS}})
        writer.writerow({"dataset": "d1", **{c: "ok" for c in smoke.COLUMNS},
                         **{c: "not this run (last: 20261019T030000Z ko)" for c in smoke.ROTATING}})
    # 2 inventory + 3 × 8 core requests shared nothing (types, the JSON Schema and the views the
    # last three): 26, then room for one dataset's 4 slow checks.
    rows, fake = table(world(packages, ["e0", "e1", "e2"]), tmp_path=tmp_path, budget=30)
    assert smoke.verdict(rows["d2"]["csv"]) == "ok", "never checked goes first"
    assert rows["d1"]["csv"] == "not this run (last: 20261019T030000Z ko)", "the carried time stays"
    assert rows["d0"]["csv"] == "not this run (last: 20261019T050000Z ok)"


def test_a_run_writes_one_table_and_exits_one_on_a_ko(tmp_path, capsys):
    fake = Fake(world([package("air", "a1", "AirQualityObserved")], []))
    assert smoke.run(DOMAIN, "prod", tmp_path, fake.client(), "20261019T060000Z") == 1
    table_path = tmp_path / "smoke-catalogue-prod-20261019T060000Z.tsv"
    rows = list(csv.DictReader(table_path.open(), delimiter="\t"))
    assert rows[0]["inventory"] == "ko: its Endpoint is not in the public DCAT feed"
    out = capsys.readouterr().out
    assert "  FAIL  catalogue air (a1): inventory its Endpoint is not in the public DCAT feed" in out
    assert "1 datasets, 0 complete, 1 ko" in out


def test_a_missing_datastore_table_warns_and_exits_zero(tmp_path, capsys):
    """T-5060 removes the DataStore: a dataset without a table is printed as a warning, not a ko."""
    fake = Fake(world([package("air", "a1", "AirQualityObserved", datastore="")], ["a1"]))
    assert smoke.run(DOMAIN, "prod", tmp_path, fake.client(), "20261019T060000Z") == 0
    out = capsys.readouterr().out
    assert "  warn  catalogue air (a1): datastore no datastore table" in out
    assert "1 datasets, 1 complete, 0 ko" in out


def test_a_clean_run_exits_zero(tmp_path, capsys):
    fake = Fake(world([package("air", "a1", "AirQualityObserved")], ["a1"]))
    assert smoke.run(DOMAIN, "prod", tmp_path, fake.client(), "20261019T060000Z") == 0
    assert "1 datasets, 1 complete, 0 ko" in capsys.readouterr().out


@pytest.mark.parametrize("status", [500, 0])
def test_an_unreadable_catalogue_stops_the_run_red(status, tmp_path):
    fake = Fake({})
    fake.answers[f"{CKAN}/api/3/action/package_search?rows=1000&start=0"] = answer(status, error="x" if not status else "")
    with pytest.raises(SystemExit, match="package_search answered"):
        smoke.run(DOMAIN, "prod", tmp_path, fake.client(), "t")


def test_a_rate_limit_that_lasts_is_not_this_run_never_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    url = f"{API}/a1/mcp"
    rows, fake = table(world([p], ["a1"], overrides={url: answer(429, retry_after="2")}))
    assert rows["air"]["mcp"] == "not this run (never)"
    assert [u for _, u in fake.sent].count(url) == 1 + smoke.RETRIES
    assert not any(smoke.verdict(rows["air"][c]) == "ko" for c in smoke.COLUMNS)


def test_a_503_that_lasts_is_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    rows, _ = table(world([p], ["a1"], overrides={f"{API}/a1/schema/index.json": answer(503)}))
    assert rows["air"]["model"].startswith("ko: schema/index.json answered 503")


def test_slow_answers_still_end_inside_ten_minutes():
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(200)]
    fake = Fake(world(packages, [f"e{i}" for i in range(200)]))
    send = fake.send

    def slow(*args):
        fake.now += 19.0
        return send(*args)

    client = smoke.Client(slow, fake.ws, fake.clock, fake.sleep)
    datasets, public = smoke.inventory(client, DOMAIN)
    smoke.Run(client, DOMAIN, {}, "", "now").table(datasets, public)
    assert fake.now <= smoke.DEADLINE


def test_a_run_that_ends_early_starts_the_next_with_the_datasets_it_missed(tmp_path):
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(3)]
    previous = tmp_path / "smoke-catalogue-prod-20261019T050000Z.tsv"
    with previous.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, ["dataset", "endpoint", "type", *smoke.COLUMNS], delimiter="\t")
        writer.writeheader()
        for name in ("d0", "d1"):
            writer.writerow({"dataset": name, **{c: "ok" for c in smoke.COLUMNS}})
        writer.writerow({"dataset": "d2", **{c: "not this run (never)" for c in smoke.COLUMNS}})
    rows, _ = table(world(packages, ["e0", "e1", "e2"]), tmp_path=tmp_path, budget=7)
    assert smoke.verdict(rows["d2"]["ngsi-ld"]) == "ok"
    assert rows["d0"]["ngsi-ld"] == "not this run (last: 20261019T050000Z ok)"


def test_a_dataset_without_a_type_reads_the_endpoint_s_types_and_names_a_failure():
    p = package("stats", "v1", "Thing")
    p["resources"] = [dict(r, url=r["url"].rsplit("/odata/v4/", 1)[0] + "/odata/v4/" if "odata" in r["url"] else r["url"],
                           name="") for r in p["resources"] if r["format"] != "OGCFEAT"]
    types = f"{API}/v1/ngsi-ld/v1/types"
    search = {f"{CKAN}/api/3/action/package_search?rows=1000&start=0":
              answer(body={"result": {"count": 1, "results": [p]}})}
    healthy = world([package("stats", "v1", "Thing", ogc=False)], ["v1"])
    rows, _ = table({**healthy, **search, types: answer(body={"typeList": ["Thing"]})})
    assert rows["stats"]["type"] == "Thing" and smoke.verdict(rows["stats"]["odata"]) == "ok"
    rows, _ = table({**healthy, **search, types: answer(500)})
    assert rows["stats"]["ngsi-ld"] == "ko: no entity type: types answered 500 application/json"


def test_a_trickling_answer_is_cut_at_the_timeout_in_total(monkeypatch):
    import socket
    import threading
    import time

    monkeypatch.setattr(smoke, "TIMEOUT", 1.0)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)

    def serve():
        conn, _ = listener.accept()
        conn.recv(4096)
        time.sleep(0.6)  # a slow first byte, then a trickle: neither alone reaches the timeout
        conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 100000\r\n\r\n")
        for _ in range(30):
            try:
                conn.sendall(b"x" * 1000)
                time.sleep(0.2)
            except OSError:
                break
        conn.close()
        listener.close()

    threading.Thread(target=serve, daemon=True).start()
    started = time.monotonic()
    got = smoke.send("GET", f"http://127.0.0.1:{listener.getsockname()[1]}/", {}, None)
    assert got.error == "timeout after 1 s" and time.monotonic() - started < 3


def test_the_tables_live_where_an_apply_leaves_them_and_git_ignores_them():
    root = SCRIPT.parent.parent
    justfile = (root / "justfile").read_text()
    removed = [line.split("rm -rf", 1)[1].split() for line in justfile.splitlines() if "rm -rf" in line]
    assert all(smoke.DEFAULT_OUT not in words and "." not in words for words in removed)
    assert f"/{smoke.DEFAULT_OUT}/" in (root / ".gitignore").read_text().splitlines()


def test_the_summary_says_the_real_duration(tmp_path, capsys):
    fake = Fake(world([package("air", "a1", "AirQualityObserved")], ["a1"]))
    client = fake.client()
    client.elapsed = lambda: 580.0  # 9 min 40 s, which a rounded minute printed as "10 min 40 s"
    smoke.run(DOMAIN, "prod", tmp_path, client, "t")
    assert " in 9 min " in capsys.readouterr().out


def test_a_trickling_error_body_is_cut_at_the_timeout_too(monkeypatch):
    import socket
    import threading
    import time

    monkeypatch.setattr(smoke, "TIMEOUT", 1.0)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)

    def serve():
        conn, _ = listener.accept()
        conn.recv(4096)
        conn.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100000\r\n\r\n")
        for _ in range(30):
            try:
                conn.sendall(b"x" * 1000)
                time.sleep(0.2)
            except OSError:
                break
        conn.close()
        listener.close()

    threading.Thread(target=serve, daemon=True).start()
    started = time.monotonic()
    got = smoke.send("GET", f"http://127.0.0.1:{listener.getsockname()[1]}/", {}, None)
    assert got.error == "timeout after 1 s" and time.monotonic() - started < 3


# The data check (T-4743): the rows themselves, against their model, name and region.

def data_cell(rows_body=None, schema=None, **pkg):
    p = package("schools", "o1", "Organization", **pkg)
    data = smoke.describe(p, DOMAIN)
    overrides = {}
    if rows_body is not None:
        overrides[f"{API}/o1/ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data))] = answer(
            body=rows_body, ctype="application/ld+json")
    if schema is not None:
        overrides[f"{API}/o1/schema/v1/model.schema.json"] = answer(body=schema)
    rows, _ = table(world([p], ["o1"], overrides=overrides))
    return rows["schools"]


def test_a_clean_dataset_passes_the_data_check_inside_its_region():
    assert data_cell()["data"] == "ok: 2 rows, inside SK032"


def test_a_municipality_row_in_organization_is_a_ko_even_when_its_model_lists_the_count():
    town = row("Organization", 419, pocetObyvatelov={"type": "Property", "value": 599})
    cell = data_cell([row("Organization"), town], model("Organization", "pocetObyvatelov"))
    assert cell["data"] == ("ko: urn:ngsi-ld:Organization:419 has pocetObyvatelov, "
                            "a place's count, not an Organization's")
    assert "599" not in cell["data"], "the table names the attribute, never its value"


def test_an_attribute_the_model_does_not_define_and_a_missing_required_one_are_ko():
    cell = data_cell([row("Organization", stray={"type": "Property", "value": 1})],
                     model("Organization", "legalId", required=["legalId"]))
    assert cell["data"] == ("ko: urn:ngsi-ld:Organization:1 has no legalId; "
                            "urn:ngsi-ld:Organization:1 has stray, which the Organization model does not define")


@pytest.mark.parametrize("name", [{"type": "Property", "value": "  "},
                                  {"type": "LanguageProperty", "languageMap": {"sk": "", "en": " "}}])
def test_a_blank_name_is_a_ko(name):
    bare = row("Organization", 2, name=name)
    assert data_cell([row("Organization"), bare])["data"] == "ko: urn:ngsi-ld:Organization:2 has no name"


def test_a_missing_name_is_a_ko_only_when_the_model_requires_one():
    """IPR's green register has no name field (T-6073): its Trees and Gardens leave `name` out."""
    bare = row("Organization", 2)
    bare.pop("name")
    assert data_cell([row("Organization"), bare])["data"] == "warn: 2 rows, 1 without name"
    required = model("Organization", required=["name"])
    assert data_cell([row("Organization"), bare], required)["data"] == "ko: urn:ngsi-ld:Organization:2 has no name"


def test_a_location_past_the_region_inside_its_country_is_a_warn_outside_the_country_a_ko():
    """PID's fare zones and lines reach from Prague (CZ010) into Středočeský kraj and beyond (T-6073)."""
    kladno = row("Organization", 3, location={"type": "GeoProperty", "value": {
        "type": "LineString", "coordinates": [[14.42, 50.08], [14.10, 50.14]]}})
    prague = row("Organization", 4, location={"type": "GeoProperty",
                                              "value": {"type": "Point", "coordinates": [14.42, 50.08]}})
    assert data_cell([prague, kladno], region="CZ010")["data"] == "warn: 2 rows, 1 reach beyond CZ010 inside CZ0"
    swapped = row("Organization", 5, location={"type": "GeoProperty",
                                               "value": {"type": "Point", "coordinates": [50.08, 14.42]}})
    assert data_cell([prague, swapped], region="CZ010")["data"] == (
        "ko: urn:ngsi-ld:Organization:5 location lies outside CZ010")
    # A country's own box has no wider one.
    assert data_cell([swapped], region="SK0")["data"] == "ko: urn:ngsi-ld:Organization:5 location lies outside SK0"


def test_a_name_in_one_language_is_enough():
    named = row("Organization", name={"type": "LanguageProperty", "languageMap": {"sk": "", "fi": "Koulu"}})
    assert smoke.verdict(data_cell([named])["data"]) == "ok"


def test_a_shared_id_and_a_row_of_another_type_are_ko():
    cell = data_cell([row("Organization"), row("Organization"), row("PublicOrganization", 3)])
    assert cell["data"] == ("ko: urn:ngsi-ld:Organization:1 twice; "
                            "urn:ngsi-ld:PublicOrganization:3 is a PublicOrganization")


def test_a_point_outside_the_region_is_a_ko_and_a_parent_code_is_used():
    prague = row("Organization", 7, location={"type": "GeoProperty",
                                              "value": {"type": "Point", "coordinates": [14.42, 50.08]}})
    assert data_cell([row("Organization"), prague])["data"] == (
        "ko: urn:ngsi-ld:Organization:7 location lies outside SK032")
    # SK0323 (an okres) has no box of its own: its kraj's box holds.
    assert data_cell([prague], region="SK0323")["data"] == "ko: urn:ngsi-ld:Organization:7 location lies outside SK032"
    line = row("Organization", 8, location={"type": "GeoProperty", "value": {
        "type": "LineString", "coordinates": [[19.1, 48.7], [24.9, 60.2]]}})
    assert data_cell([line])["data"] == "ko: urn:ngsi-ld:Organization:8 location lies outside SK032"
    broken = row("Organization", 9, location={"type": "GeoProperty", "value": {"type": "Point"}})
    assert data_cell([broken])["data"] == "ko: urn:ngsi-ld:Organization:9 location is no GeoJSON geometry"


def test_a_dataset_without_a_region_takes_its_organization_s_or_says_it_did_not_check():
    a = package("schools", "o1", "Organization", region="", org="bbsk")
    b = package("roads", "o2", "Road", org="bbsk")
    rows, _ = table(world([a, b], ["o1", "o2"]))
    assert rows["schools"]["data"] == "ok: 2 rows, inside SK032"
    alone, _ = table(world([package("schools", "o1", "Organization", region="", org="x")], ["o1"]))
    assert alone["schools"]["data"] == "ok: 2 rows, location not checked: no region"
    far, _ = table(world([package("schools", "o1", "Organization", region="DE300")], ["o1"]))
    assert far["schools"]["data"] == "ok: 2 rows, location not checked: no box for DE300"


def test_an_empty_dataset_is_a_ko_with_no_rows():
    cell = data_cell([])
    assert cell["data"] == "ko: no rows"
    assert cell["ngsi-ld"] == "ko: no Organization entity"


def test_more_than_three_problems_are_counted_not_listed():
    rows_body = [row("Organization", n, name={"type": "Property", "value": ""}) for n in range(5)]
    assert data_cell(rows_body)["data"].endswith("urn:ngsi-ld:Organization:2 has no name (+2 more)")


def test_a_missing_json_schema_is_a_ko():
    p = package("schools", "o1", "Organization")
    rows, _ = table(world([p], ["o1"], overrides={f"{API}/o1/schema/v1/model.schema.json": answer(404)}))
    assert rows["schools"]["data"] == "ko: model.schema.json answered 404 application/json, no Organization"


def test_a_located_dataset_without_ogc_is_a_ko_on_ogc_and_map():
    p = package("stops", "o1", "Organization", ogc=False)
    data = smoke.describe(p, DOMAIN)
    url = f"{API}/o1/ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data))
    rows, _ = table(world([p], ["o1"], overrides={url: answer(body=[row("Organization")], ctype=JSON)}))
    assert rows["stops"]["ogc"] == "ko: its rows carry a location, but the dataset lists no OGC distribution"
    assert rows["stops"]["map"] == "ko: its rows carry a location, but there is no OGC distribution to map"


def test_a_map_without_a_feature_is_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    rows, _ = table(world([p], ["a1"], overrides={
        f"{API}/a1/ogc/features/collections/AirQualityObserved/items?limit=1": answer(body={"features": []}, ctype=GEO)}))
    assert rows["air"]["map"] == "ko: no feature to show, OGC no AirQualityObserved feature"


def test_the_sample_reads_twenty_rows_of_the_dataset_s_own_is_part_of():
    data = smoke.Dataset("x", type="Road", part_of="https://x/dataset/three")
    assert smoke.sample_query(data) == {"type": "Road", "limit": 20, "q": 'isPartOf=="https://x/dataset/three"'}



def filtered(name, slug, typ, part_of):
    """A dataset whose CKAN links keep its rows by isPartOf, as od-standards.py writes them."""
    p = package(name, slug, typ, ogc=False)
    for r in p["resources"]:
        if r["format"] == "NGSI-LD" or r["url"].endswith("file.csv"):
            r["url"] += "?" + urllib.parse.urlencode({"type": typ, "q": f'isPartOf=="{part_of}"'})
        elif r["format"] == "OData":
            r["url"] += "?" + urllib.parse.urlencode({"$filter": f"isPartOf eq '{part_of}'"})
    return p


def test_a_dataset_without_ogc_takes_its_is_part_of_from_any_link():
    """T-4908: CKAN linked opendata.{domain} while the rows named data.{domain}; without an OGC
    link the smoke read the whole type and missed the empty download."""
    data = smoke.describe(filtered("schools", "o1", "Organization", "https://x/dataset/o'k"), DOMAIN)
    assert data.part_of == "https://x/dataset/o'k"
    assert smoke.is_part_of("https://a/ogc/features/collections/T/items?isPartOf=https%3A%2F%2Fx%2Fd") == "https://x/d"
    assert smoke.is_part_of("https://a/odata/v4/T?%24filter=isPartOf+eq+%27a%27%27b%27") == "a'b"
    assert smoke.is_part_of("https://a/ngsi-ld/v1/entities?type=T&q=name%3D%3D%22x%22") == ""


def test_an_empty_download_of_the_dataset_s_rows_is_a_ko_on_every_interface():
    p = filtered("schools", "o1", "Organization", "https://opendata.example.org/dataset/schools")
    data = smoke.describe(p, DOMAIN)
    base = f"{API}/o1/"
    rows, fake = table(world([p], ["o1"], overrides={
        base + "ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data)): answer(body=[]),
        base + "odata/v4/Organization?" + urllib.parse.urlencode(smoke.odata_query(data)):
            answer(body={"@odata.count": 0, "value": []}),
        base + "file.csv?" + urllib.parse.urlencode(smoke.sample_query(data, 1)): answer(body=b"id,type\n", ctype=CSV),
    }))
    row = rows["schools"]
    assert row["ngsi-ld"] == "ko: no Organization entity"
    assert row["odata"] == "ko: @odata.count is 0"
    assert row["csv"] == "ko: file.csv has no Organization row"
    sent = [url for _, url in fake.sent]
    assert any("file.csv?" in u and "isPartOf" in urllib.parse.unquote_plus(u) for u in sent), "the CKAN link's own filter"
    healthy, _ = table(world([p], ["o1"]))
    assert [smoke.verdict(healthy["schools"][c]) for c in ("ngsi-ld", "odata", "csv")] == ["ok"] * 3


# The owner's addendum (T-4743, 17:10Z): relationships, every served type modelled, CKAN views.

def test_a_dangling_relationship_is_a_ko_with_its_attribute():
    poi = "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-41960"
    beach = row("Organization", 1, refPointOfInterest={"type": "Relationship", "object": poi})
    p = package("schools", "o1", "Organization")
    data = smoke.describe(p, DOMAIN)
    sample = f"{API}/o1/ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data))
    lookup = f"{API}/o1/ngsi-ld/v1/entities?" + urllib.parse.urlencode({"type": "PointOfInterest", "id": poi, "limit": 1})
    schema = model("Organization", "refPointOfInterest")
    over = {sample: answer(body=[beach], ctype=JSON), f"{API}/o1/schema/v1/model.schema.json": answer(body=schema)}
    rows, _ = table(world([p], ["o1"], overrides={**over, lookup: answer(body=[], ctype=JSON)}))
    assert rows["schools"]["data"] == "ko: urn:ngsi-ld:Organization:1 refPointOfInterest points to no entity a public Endpoint holds"
    rows, _ = table(world([p], ["o1"], overrides={**over, lookup: answer(body=[{"id": poi}], ctype=JSON)}))
    assert rows["schools"]["data"] == "ok: 1 rows, inside SK032"
    # A register's Endpoint serves its one type; the place it names sits behind the places' dataset (T-4745).
    places = package("places", "p1", "PointOfInterest", org="helsinki")
    elsewhere = [package(f"places-{n}", f"x{n}", "PointOfInterest", org="praha") for n in range(3)]
    p["organization"] = {"name": "helsinki"}
    there = f"{API}/p1/ngsi-ld/v1/entities?" + urllib.parse.urlencode({"type": "PointOfInterest", "id": poi, "limit": 1})
    rows, _ = table(world([p, *elsewhere, places], ["o1", "p1", "x0", "x1", "x2"], overrides={
        **over, lookup: answer(body=[], ctype=JSON), there: answer(body=[{"id": poi}], ctype=JSON)}))
    assert rows["schools"]["data"] == "ok: 1 rows, inside SK032"
    # A dataset whose Endpoint is not public holds nothing a visitor can follow.
    rows, _ = table(world([p, places], ["o1"], overrides={
        **over, lookup: answer(body=[], ctype=JSON), there: answer(body=[{"id": poi}], ctype=JSON)}))
    assert rows["schools"]["data"].startswith("ko: urn:ngsi-ld:Organization:1 refPointOfInterest points to no entity")


def test_a_relationship_that_is_no_urn_is_a_ko():
    odd = row("Organization", 1, refPublisher={"type": "Relationship", "object": "bbsk"})
    cell = data_cell([odd], model("Organization", "refPublisher"))
    assert cell["data"] == "ko: urn:ngsi-ld:Organization:1 refPublisher is no NGSI-LD URN"
    many = row("Organization", 2, refArea={"type": "Relationship", "object": [
        "urn:ngsi-ld:AdministrativeArea:a", "urn:ngsi-ld:AdministrativeArea:b"]})
    assert smoke.relationships([many]) == {"AdministrativeArea": {
        "urn:ngsi-ld:AdministrativeArea:a": ("urn:ngsi-ld:Organization:2", "refArea"),
        "urn:ngsi-ld:AdministrativeArea:b": ("urn:ngsi-ld:Organization:2", "refArea")}}


def test_a_type_the_endpoint_serves_without_a_model_is_a_ko():
    p = package("air", "a1", "AirQualityObserved")
    rows, _ = table(world([p], ["a1"], overrides={
        f"{API}/a1/ngsi-ld/v1/types": answer(body={"typeList": ["AirQualityObserved", "Device"]})}))
    assert rows["air"]["model"] == "ko: served without a model: Device"


def series_model(dated="date-time"):
    """A WaterQualityObserved model jcctl draws a chart of: a number and a date-time dateObserved."""
    schema = model("WaterQualityObserved", "temperature", "dateObserved")
    properties = schema["definitions"]["WaterQualityObserved"]["properties"]
    properties["temperature"] = {"type": "number", "x-ngsi-ld-kind": "Property"}
    properties["dateObserved"] = {"type": "string", "format": dated, "x-ngsi-ld-kind": "Property"}
    return schema


def series_dataset(views, first=(), schema=None):
    p = package("beach", "w1", "WaterQualityObserved")
    data = smoke.describe(p, DOMAIN)
    observed = row("WaterQualityObserved", 1, temperature={"type": "Property", "value": 17.5},
                   dateObserved={"type": "Property", "value": "2026-10-19T06:00:00Z"})
    rows, _ = table(world([p], ["w1"], overrides={
        f"{API}/w1/ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data)): answer(body=[observed], ctype=JSON),
        f"{API}/w1/schema/v1/model.schema.json": answer(body=schema or series_model()),
        f"{CKAN}/api/3/action/resource_view_list?id=ds-beach": answer(body={"result": views}),
        f"{CKAN}/api/3/action/resource_view_list?id=ngsi-beach": answer(body={"result": list(first)})}))
    return rows["beach"]["views"]


def test_a_water_quality_dataset_without_a_chart_view_is_a_ko():
    assert series_dataset([{"view_type": "datatables_view"}]) == "ko: a numeric value over time and no chart view"
    assert series_dataset([{"view_type": "datatables_view"}, {"view_type": "jc_visualization"}]) == "ok: table, chart"


def test_the_chart_jcctl_hangs_off_the_first_resource_counts():
    """publish_views puts every chart on the dataset's first resource, the NGSI-LD one (T-4745: 7 dev
    datasets read ko with their charts in place)."""
    chart = [{"view_type": "jc_visualization"}]
    assert series_dataset([{"view_type": "datatables_view"}], first=chart) == "ok: table, chart"


def test_a_model_without_a_dated_number_owes_no_chart():
    """jcctl's series_charts draws only a number beside a date-time dateObserved; the smoke asks the same."""
    assert series_dataset([{"view_type": "datatables_view"}], schema=series_model(dated="date")) == (
        "ok: table; no chart: the WaterQualityObserved model has no number observed at a date-time dateObserved")


def test_every_datastore_table_needs_a_table_view():
    assert series_dataset([{"view_type": "jc_visualization"}]) == "ko: no table view on 1 of 1 datastore tables"


def test_a_register_without_a_value_over_time_needs_only_the_table():
    rows, _ = table(world([package("schools", "o1", "Organization")], ["o1"]))
    assert rows["schools"]["views"] == "ok: table; no chart: the Organization model has no number observed at a date-time dateObserved"
    nothing, _ = table(world([package("schools", "o1", "Organization", datastore="")], ["o1"]))
    assert nothing["schools"]["views"] == "skip: no datastore table"


def test_a_count_without_a_time_is_no_series():
    beach = series_model()["definitions"]["WaterQualityObserved"]
    assert smoke.series(beach)
    assert not smoke.series({"properties": {"pupils": {"type": "integer", "x-ngsi-ld-kind": "Property"}}})
    flag = {"type": "boolean", "x-ngsi-ld-kind": "Property"}
    assert not smoke.series({"properties": {**beach["properties"], "temperature": flag}})


def test_a_holder_whose_type_only_its_endpoint_names_is_asked_whatever_the_check_order():
    # T-5402: helsinki-districts names its type only on /types; the permits checked first saw no holder.
    poi = "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-41960"
    p = package("schools", "o1", "Organization", org="helsinki")
    places = package("places", "p1", "PointOfInterest", ogc=False, org="helsinki")
    places["resources"] = [dict(r, url=r["url"].rsplit("/odata/v4/", 1)[0] + "/odata/v4/" if "odata" in r["url"] else r["url"],
                                name="") for r in places["resources"]]
    data = smoke.describe(p, DOMAIN)
    sample = f"{API}/o1/ngsi-ld/v1/entities?" + urllib.parse.urlencode(smoke.sample_query(data))
    query = urllib.parse.urlencode({"type": "PointOfInterest", "id": poi, "limit": 1})
    healthy = world([p, package("places", "p1", "PointOfInterest", ogc=False, org="helsinki")], ["o1", "p1"])
    search = {f"{CKAN}/api/3/action/package_search?rows=1000&start=0":
              answer(body={"result": {"count": 2, "results": [p, places]}})}
    rows, _ = table({**healthy, **search,
                     sample: answer(body=[row("Organization", 1, refPointOfInterest={"type": "Relationship", "object": poi})]),
                     f"{API}/o1/schema/v1/model.schema.json": answer(body=model("Organization", "refPointOfInterest")),
                     f"{API}/o1/ngsi-ld/v1/entities?{query}": answer(body=[]),
                     f"{API}/p1/ngsi-ld/v1/entities?{query}": answer(body=[{"id": poi}])})
    assert rows["schools"]["data"] == "ok: 1 rows, inside SK032"
    assert rows["places"]["type"] == "PointOfInterest"


def _filters(count, formats, topics=(("TRAN", 3),), status=200):
    facets = {"vocab_jc_format": {"items": [{"name": n, "count": c} for n, c in formats]},
              "vocab_jc_topic": {"items": [{"name": n, "count": c} for n, c in topics]}}
    fake = Fake({FILTERS: answer(status, {"result": {"count": count, "search_facets": facets}})})
    return smoke.filters(fake.client(), DOMAIN)


@pytest.mark.parametrize("formats, topics, cell", [
    # T-5649 on dev: NGSI-LD on all 140 datasets is a served format and stays.
    ([("NGSI-LD", 140), ("WEBSOCKET", 95), ("STA", 3)], [("TRAN", 18)], "ok: 3 formats, 1 topics"),
    # T-5294 put the data model's documents out of the filter, T-5649 the translated uses.
    ([("CSV", 140), ("OWL", 139)], [("TRAN", 18)], "ko: the Format filter names OWL"),
    ([("map", 115), ("live", 95)], [("TRAN", 18)], "ko: the Format filter names live, map"),
    ([], [("TRAN", 18)], "ko: the Format filter offers no choice"),
    ([("NGSI-LD", 140)], [], "ko: the Topic filter is missing"),
])
def test_the_search_filters_name_the_served_formats(formats, topics, cell):
    assert _filters(140, formats, topics) == cell


def test_an_empty_catalogue_skips_the_filters_and_an_error_is_a_ko():
    assert _filters(0, []) == "skip: no dataset"
    assert _filters(107, [], status=500).startswith("ko: package_search with the filters answered")


def test_a_run_writes_the_filters_as_a_row_of_their_own(tmp_path, capsys):
    answers = world([package("air", "a1", "AirQualityObserved")], ["a1"])
    answers[FILTERS] = answer(body={"result": {"count": 1, "search_facets": {}}})
    assert smoke.run(DOMAIN, "prod", tmp_path, Fake(answers).client(), "20261019T060000Z") == 1
    out = capsys.readouterr().out
    assert "  FAIL  catalogue (catalogue) (-): inventory the Format filter offers no choice" in out
    assert "1 datasets, 1 complete, 0 ko, 0 public Endpoints without a dataset" in out


def test_a_spent_budget_still_writes_the_filters_row(tmp_path, capsys):
    # The rotating checks spend the budget; the filters run first, so a full budget never crashes the run.
    packages = [package(f"d{i}", f"e{i}", "Thing") for i in range(5)]
    answers = world(packages, [f"e{i}" for i in range(5)])
    answers[FILTERS] = answer(body={"result": {"count": 5, "search_facets": {}}})
    smoke.run(DOMAIN, "dev", tmp_path, Fake(answers).client(budget=12), "20261023T090000Z")
    assert "catalogue (catalogue) (-): inventory the Format filter offers no choice" in capsys.readouterr().out


def count_world(packages, public, empty=()):
    """world() of 4 entities a dataset, the `empty` ones none."""
    answers = world(packages, public, rows=4)
    for p in packages:
        if p["name"] in empty:
            data = smoke.describe(p, DOMAIN)
            answers[f"{API}/{data.slug}/ngsi-ld/v1/entities?" + urllib.parse.urlencode(count_query(data))] = answer(
                body=[], ctype="application/ld+json", results="0")
    return answers


def counted(answers):
    fake = Fake(answers)
    datasets, _ = smoke.inventory(fake.client(), DOMAIN)
    counter = smoke.Counts(DOMAIN, fake.send, fake.clock, fake.sleep)
    return counter.cells(datasets), fake, counter


def test_an_empty_dataset_fails_the_count_and_a_full_one_passes():
    cells, _, _ = counted(count_world([package("air", "a1", "AirQualityObserved"),
                                       package("kpi", "k1", "KeyPerformanceIndicator")], ["a1", "k1"], empty={"kpi"}))
    assert cells["air"] == "ok: 4 entities"
    assert cells["kpi"] == "ko: 0 entities of KeyPerformanceIndicator"


def test_a_429_backs_off_the_count_pass_and_the_retry_counts():
    p = package("air", "a1", "AirQualityObserved")
    answers = count_world([p], ["a1"])
    url = next(u for u in answers if "&count=true" in u)
    answers[url] = [answer(429, retry_after="7"), answers[url]]
    cells, fake, counter = counted(answers)
    assert cells["air"] == "ok: 4 entities"
    assert 7 in fake.pauses and counter.count == 2
    assert counter.gate.until == fake.now - 7 + smoke.COUNT_COOL  # 2 at once for the next minute


def test_the_count_pass_starts_at_most_400_reads_a_minute():
    # T-5903: 6 at once on the edge cache passed the edge's 600 a minute per address, and a lasting 429
    # ended dev-smoke's source links after 1218 of ~1400 reads.
    fake = Fake({})
    counter = smoke.Counts(DOMAIN, fake.send, fake.clock, fake.sleep)
    for n in range(5):
        counter.get(f"{API}/e{n}/ngsi-ld/v1/types")
    assert fake.pauses == [pytest.approx(0.15)] * 4
    assert fake.now == pytest.approx(0.6)


def test_a_429_that_lasts_leaves_the_count_not_this_run_and_a_503_that_lasts_is_a_ko():
    a, b = package("air", "a1", "AirQualityObserved"), package("park", "p1", "OnStreetParking")
    answers = count_world([a, b], ["a1", "p1"])
    urls = {n: next(u for u in answers if "&count=true" in u and f"/{s}/" in u) for n, s in (("air", "a1"), ("park", "p1"))}
    answers[urls["air"]], answers[urls["park"]] = answer(429, retry_after="1"), answer(503)
    cells, _, _ = counted(answers)
    assert "air" not in cells
    assert cells["park"].startswith("ko: the count of OnStreetParking answered 503")


def test_an_untyped_dataset_counts_the_endpoint_s_types_first_with_entities_wins():
    p = package("cat", "c1", "Thing")
    p["resources"] = [{"format": "NGSI-LD", "url": f"{API}/c1/ngsi-ld/v1/entities", "name": "NGSI-LD API"}]
    base = f"{API}/c1/ngsi-ld/v1/"
    cells, _, _ = counted({
        f"{CKAN}/api/3/action/package_search?rows=1000&start=0": answer(body={"result": {"count": 1, "results": [p]}}),
        f"https://api.{DOMAIN}/catalog.jsonld": answer(body={"dcat:dataset": []}),
        base + "types": answer(body={"typeList": ["A", "B"]}),
        base + "entities?type=A&limit=1&count=true": answer(body=[], results="0"),
        base + "entities?type=B&limit=1&count=true": answer(body=[{}], results="12"),
    })
    assert cells["cat"] == "ok: 12 entities"


def test_an_untyped_dataset_takes_its_types_from_the_models_before_the_slow_types_read():
    p = package("cat", "c1", "Thing")
    p["resources"] = [{"format": "NGSI-LD", "url": f"{API}/c1/ngsi-ld/v1/entities", "name": "NGSI-LD API"}]
    answers = {
        f"{CKAN}/api/3/action/package_search?rows=1000&start=0": answer(body={"result": {"count": 1, "results": [p]}}),
        f"https://api.{DOMAIN}/catalog.jsonld": answer(body={"dcat:dataset": []}),
        f"{API}/c1/schema/index.json": answer(body={"models": [{"types": ["Any", "Garden"]}]}),
        f"{API}/c1/ngsi-ld/v1/types": answer(503),
        f"{API}/c1/ngsi-ld/v1/entities?type=Garden&limit=1&count=true": answer(body=b"[{\"cut", results="3"),
    }
    cells, fake, _ = counted(answers)
    assert cells["cat"] == "ok: 3 entities", "the count header holds when the body outgrew MAX_BODY"
    assert f"{API}/c1/ngsi-ld/v1/types" not in [u for _, u in fake.sent]


def test_a_run_goes_red_on_an_empty_dataset_and_green_once_it_holds_data(tmp_path, capsys):
    p = package("kpi", "k1", "KeyPerformanceIndicator")
    assert smoke.run(DOMAIN, "prod", tmp_path, Fake(count_world([p], ["k1"], empty={"kpi"})).client(), "t1") == 1
    assert "  FAIL  catalogue kpi (k1): count 0 entities of KeyPerformanceIndicator" in capsys.readouterr().out
    assert smoke.run(DOMAIN, "prod", tmp_path, Fake(count_world([p], ["k1"])).client(), "t2") == 0


def test_a_flood_zone_over_max_body_still_counts_on_odata_and_has_a_feature_on_ogc():
    """praha-zaplavova-uzemi on prod (T-6073): one Q100 polygon is 1.2 MB, past MAX_BODY."""
    p = package("flood", "f1", "FloodZone")
    data = smoke.describe(p, DOMAIN)
    polygon = {"type": "Polygon", "coordinates": [[[14.4, 50.1]] * 80_000]}
    big = answer(body={"features": [{"type": "Feature", "geometry": polygon}]}, ctype=GEO)
    assert len(big.body) > smoke.MAX_BODY
    full = {"@odata.count": 2438, "value": [{"id": "urn:ngsi-ld:FloodZone:1", "location": polygon}]}
    unselected = f"{API}/f1/odata/v4/FloodZone?" + urllib.parse.urlencode({"$top": 1, "$count": "true"})
    rows, fake = table(world([p], ["f1"], overrides={
        f"{API}/f1/ogc/features/collections/FloodZone/items?limit=1": big,
        unselected: answer(body=full),
        f"{API}/f1/odata/v4/FloodZone?" + urllib.parse.urlencode(smoke.odata_query(data)):
            answer(body={"@odata.count": 2438, "value": [{"id": "urn:ngsi-ld:FloodZone:1"}]})}))
    assert rows["flood"]["odata"] == "ok: 2438 rows"
    assert rows["flood"]["ogc"] == "ok"
    assert unselected not in [u for _, u in fake.sent], "the count reads only the id"
