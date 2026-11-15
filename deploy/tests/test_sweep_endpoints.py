"""scripts/sweep-endpoints.py against recorded answers (T-6502): a 5xx or a missing link fails the
run, an empty type is named, a 429 and a 503 with Retry-After wait, and nothing is written."""

import csv
import importlib.util
import json
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/sweep-endpoints.py"
spec = importlib.util.spec_from_file_location("sweep_endpoints", SCRIPT)
sweep = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sweep)
smoke = sweep.smoke

DOMAIN = "smartcity.example.org"
SLUG = "a" * 26
BASE = f"https://api.{DOMAIN}/api/endpoint/{SLUG}"
TYPE = "AirQualityObserved"


def answer(status=200, body=b"{}", **kw):
    if not isinstance(body, bytes):
        body = json.dumps(body).encode()
    return smoke.Answer(status, "application/json", body, **kw)


def healthy() -> dict:
    links = ["ngsi-ld/v1/", "file.csv", "ws", "ogc/features/", "odata/v4/", "mcp", "schema/v1/model.md"]
    return {
        f"https://api.{DOMAIN}/catalog.jsonld": answer(body={"dcat:dataset": [{"@id": BASE}]}),
        BASE: answer(body={"dcat:distribution": [{"dcat:accessURL": f"{BASE}/{x}"} for x in links]}),
        f"{BASE}/ngsi-ld/v1/types": answer(body={"typeList": [TYPE]}),
        f"{BASE}/schema/index.json": answer(body={"models": [{"types": [TYPE, "Any"]}]}),
        f"{BASE}/schema/v1/model.md": answer(),
        f"{BASE}/odata/v4": answer(body={"value": [{"name": TYPE}]}),
        f"{BASE}/ogc/features/collections": answer(body={"collections": [{"id": TYPE}]}),
        f"{BASE}/ngsi-ld/v1/entities?type={TYPE}&count=true&limit=1": answer(body=[{}], results="7"),
        f"{BASE}/odata/v4/{TYPE}?%24top=1&%24count=true": answer(body={"@odata.count": 7}),
        f"{BASE}/ogc/features/collections/{TYPE}/items?limit=1": answer(body={"numberMatched": 7, "features": [{}]}),
        f"{BASE}/file.csv?type={TYPE}&limit=1": smoke.Answer(200, "text/csv", b"id\nurn:x\n"),
    }


class Fake:
    def __init__(self, answers):
        self.answers, self.seen = answers, []

    def __call__(self, method, url, headers, body, limit):
        self.seen.append((method, url))
        if url.endswith("/mcp"):
            call = json.loads(body)
            if call["method"] == "tools/list":
                return answer(body={"result": {"tools": [{"name": "query_entities"}]}})
            if call["params"]["name"] == "list_endpoints":
                return answer(body={"result": {"structuredContent": {"endpoints": []}}})
            assert call["params"]["arguments"]["type"] == [TYPE]
            return answer(body={"result": {"structuredContent": {"entities": [{}]}}})
        found = self.answers[url]
        if isinstance(found, list):
            return found.pop(0)
        return found


def walk(tmp_path, answers, ws=(101, 201, "")):
    fake = Fake(answers)
    waits = []
    reader = sweep.Reader(sweep.Pace(10 ** 9, sleep=lambda s: None), fake, waits.append, lambda url, typ: ws)
    out = tmp_path / "table.tsv"
    code = sweep.run(DOMAIN, "prod", out, reader=reader)
    with out.open() as handle:
        rows = list(csv.DictReader(handle, delimiter="\t", quoting=csv.QUOTE_NONE))
    return code, {(r["type"], r["surface"]): r for r in rows}, fake, waits


def test_a_healthy_endpoint_passes_on_every_surface_and_is_only_read(tmp_path):
    code, rows, fake, _ = walk(tmp_path, healthy())
    assert code == 0
    for surface in ("ngsi-ld", "odata", "ogc", "csv", "websocket", "mcp-read"):
        assert rows[(TYPE, surface)]["status"] == "ok", surface
    assert rows[(TYPE, "ngsi-ld")]["rows"] == "7"
    assert rows[("-", "link")]["note"] == "schema/v1/model.md"
    assert {method for method, url in fake.seen if not url.endswith("/mcp")} == {"GET"}


def test_a_5xx_on_any_surface_fails_the_run_after_one_more_read(tmp_path):
    answers = healthy()
    bad = f"{BASE}/odata/v4/{TYPE}?%24top=1&%24count=true"
    answers[bad] = answer(500, {"title": "broken"})
    code, rows, fake, waits = walk(tmp_path, answers)
    assert code == 1
    assert rows[(TYPE, "odata")]["status"] == "5xx"
    assert sum(url == bad for _, url in fake.seen) == 2
    assert waits == [sweep.SLOW_RETRY]


def test_a_503_that_recovers_passes_and_says_so(tmp_path):
    answers = healthy()
    answers[f"{BASE}/ngsi-ld/v1/types"] = [answer(500), answer(body={"typeList": [TYPE]})]
    code, rows, _, _ = walk(tmp_path, answers)
    assert code == 0
    assert "first read 500" in rows[("-", "types")]["note"]


def test_the_gateways_refusals_wait_as_long_as_they_name(tmp_path):
    answers = healthy()
    url = f"{BASE}/file.csv?type={TYPE}&limit=1"
    answers[url] = [answer(503, retry_after="4"), answer(429, retry_after="9"), answers[url]]
    code, rows, _, waits = walk(tmp_path, answers)
    assert code == 0
    assert rows[(TYPE, "csv")]["status"] == "ok"
    assert waits == [4.0, 9.0]


def test_a_broken_distribution_link_fails_the_run(tmp_path):
    answers = healthy()
    answers[f"{BASE}/schema/v1/model.md"] = answer(404)
    code, rows, _, _ = walk(tmp_path, answers)
    assert rows[("-", "link")]["status"] == "broken"
    assert code == 1


def test_an_empty_type_and_a_count_that_differs_are_named_and_do_not_fail(tmp_path):
    answers = healthy()
    answers[f"{BASE}/odata/v4/{TYPE}?%24top=1&%24count=true"] = answer(body={"@odata.count": 5})
    code, rows, _, _ = walk(tmp_path, answers)
    assert (code, rows[(TYPE, "odata")]["status"]) == (0, "differs")
    assert "NGSI-LD counts 7" in rows[(TYPE, "odata")]["note"]

    answers = healthy()
    answers[f"{BASE}/ngsi-ld/v1/entities?type={TYPE}&count=true&limit=1"] = answer(body=[], results="0")
    code, rows, _, _ = walk(tmp_path, answers)
    assert (code, rows[(TYPE, "ngsi-ld")]["status"], rows[("-", "endpoint")]["status"]) == (0, "empty", "empty")


def test_a_refused_websocket_is_read_as_its_status(tmp_path):
    answers = healthy()
    code, rows, _, _ = walk(tmp_path, answers, ws=(502, 0, ""))
    assert (code, rows[(TYPE, "websocket")]["status"]) == (1, "5xx")
