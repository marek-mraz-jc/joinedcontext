"""scripts/open-data-scoreboard.py against recorded answers (T-5275): of one passing dataset, one failing
its Smart Data Model, one stale and one with an empty format link, the city counts 1; a dataset that
passed before and fails now makes the run exit 1; the validation flags an out-of-area point, an
out-of-range value and a duplicate, each with its reason."""

import importlib.util
import json
import urllib.parse
from datetime import UTC, datetime, timedelta
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/open-data-scoreboard.py"
spec = importlib.util.spec_from_file_location("open_data_scoreboard", SCRIPT)
board = importlib.util.module_from_spec(spec)
spec.loader.exec_module(board)
smoke = board.smoke

DOMAIN = "smartcity.example.org"
API = f"https://api.{DOMAIN}/api/endpoint"
NOW = datetime(2026, 10, 22, 19, 0, tzinfo=UTC)
HOURLY = "http://publications.europa.eu/resource/authority/frequency/HOURLY"


def package(name, slug, frequency=HOURLY):
    base = f"{API}/{slug}"
    return {"name": name, "organization": {"name": "praha"},
            "extras": [{"key": "endpoint", "value": f"{base}/"}, {"key": "frequency", "value": frequency},
                       {"key": "was_generated_by", "value": f"pipeline {name}"}],
            "resources": [{"url": f"{base}/ngsi-ld/v1/", "format": "NGSI-LD", "name": "Parking (OffStreetParking)"},
                          {"url": f"{base}/file.csv", "format": "CSV"},
                          {"url": f"{base}/ws", "format": "WebSocket"},
                          {"url": f"{base}/mcp", "format": "MCP"}]}


def parking(slug, n, total=200, modified=NOW - timedelta(minutes=10), lon=14.42, lat=50.08):
    return {"id": f"urn:ngsi-ld:OffStreetParking:{slug}:{n}", "type": "OffStreetParking",
            "name": {"type": "Property", "value": f"P+R {slug} {n}"},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [lon, lat]}},
            "totalSpotNumber": {"type": "Property", "value": total},
            "availableSpotNumber": {"type": "Property", "value": 12},
            "createdAt": "2026-10-01T00:00:00Z", "modifiedAt": modified.strftime("%Y-%m-%dT%H:%M:%SZ")}


DATASETS = {
    "good": [parking("good", 1), parking("good", 2, lon=14.43)],
    "badsdm": [parking("badsdm", 1, total="many")],
    "stale": [parking("stale", 1, modified=NOW - timedelta(days=2))],
    "emptycsv": [parking("emptycsv", 1)],
}


class Recorded:
    def __init__(self, fail=(), drop=()):
        self.count, self.fail, self.drop = 0, set(fail), set(drop)

    def fetch(self, url, method="GET", headers=None, body=None, limit=0):
        self.count += 1
        parts = urllib.parse.urlsplit(url)
        query = urllib.parse.parse_qs(parts.query)
        if parts.path.endswith("package_search"):
            names = sorted(set(DATASETS) - self.drop) if query["fq"] == ['organization:"praha"'] else []
            return answer({"result": {"count": len(names), "results": [package(n, n) for n in names]}})
        slug = parts.path.split("/")[3]
        rows = DATASETS[slug] if slug not in self.fail else [parking(slug, 1, total=-1)]
        if parts.path.endswith("/ngsi-ld/v1/entities"):
            assert query["options"] == ["sysAttrs"]
            wanted = set(",".join(query.get("id", [])).split(",")) - {""}
            return answer([r for r in rows if not wanted or r["id"] in wanted])
        if "/odata/v4/" in parts.path:
            return answer({"@odata.count": len(rows)})
        if parts.path.endswith("/file.csv"):
            return smoke.Answer(200, "text/csv", b"" if slug == "emptycsv" else b"id,name\nurn:x,P\n")
        if parts.path.endswith("/ws"):
            return smoke.Answer(426, "application/problem+json", b"{}")
        if parts.path.endswith("/mcp") and method == "POST":
            return answer({"result": {"tools": [{"name": "query_entities"}]}})
        return smoke.Answer(200, "application/json", b'{"ok": true}')


def answer(body):
    return smoke.Answer(200, "application/json", json.dumps(body).encode())


def test_counts_one_of_four(tmp_path):
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(), NOW) == 0
    text = (tmp_path / "2026-10-22.md").read_text()
    assert "| praha | 1 | 100 | 99 | 4 |" in text
    rows = {line.split("|")[1].strip(): line for line in text.splitlines() if line.startswith("| ") and "pipeline" not in line.lower()}
    assert "| yes |" in rows["good"]
    assert "ko: urn:ngsi-ld:OffStreetParking:badsdm:1 totalSpotNumber" in rows["badsdm"]
    assert "ko: newest 2026-10-20T19:00Z, older than 3 x HOURLY" in rows["stale"]
    assert "ko: CSV https://api.smartcity.example.org/api/endpoint/emptycsv/file.csv is empty" in rows["emptycsv"]
    assert json.loads((tmp_path / "latest.json").read_text())["passing"] == {"praha": ["good"]}


def test_a_dataset_that_passed_before_and_fails_now_exits_1(tmp_path):
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(), NOW) == 0
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(fail={"good"}), NOW) == 1


def test_a_passing_dataset_gone_from_the_catalogue_exits_1(tmp_path):
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(), NOW) == 0
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(drop={"good"}), NOW) == 1


def test_a_subset_run_keeps_the_other_cities_verdicts(tmp_path):
    assert board.run(DOMAIN, tmp_path, ["praha"], Recorded(), NOW) == 0
    assert board.run(DOMAIN, tmp_path, ["helsinki"], Recorded(), NOW) == 0
    assert json.loads((tmp_path / "latest.json").read_text())["passing"] == {"helsinki": [], "praha": ["good"]}
    assert board.run(DOMAIN, tmp_path, list(board.CITIES), Recorded(fail={"good"}), NOW) == 1


def test_a_city_without_a_catalogue_organisation_counts_0(tmp_path):
    assert board.run(DOMAIN, tmp_path, ["barcelona", "bratislava"], Recorded(), NOW) == 0
    text = (tmp_path / "2026-10-22.md").read_text()
    assert "| barcelona | 0 | 100 | 100 | 0 |" in text
    assert "| bratislava | 0 | 100 | 100 | 0 |" in text
    assert "No CKAN dataset of barcelona yet." in text
    assert json.loads((tmp_path / "latest.json").read_text())["passing"] == {"barcelona": [], "bratislava": []}


def test_validation_flags_area_range_and_duplicate():
    box = board.widened(board.CITIES["praha"][1])
    rows = [parking("v", 1), parking("v", 1, lon=14.4201), parking("v", 3, lon=16.6, lat=49.2),
            parking("v", 4, lon=50.08, lat=14.42), parking("v", 5, lon=0, lat=0)]
    rows[1]["id"] = "urn:ngsi-ld:OffStreetParking:v:2"
    rows[2]["availableSpotNumber"]["value"] = 500
    rows[3]["totalSpotNumber"]["value"] = -3
    problems = board.validate(rows, "OffStreetParking", "praha", box, NOW)
    assert "urn:ngsi-ld:OffStreetParking:v:2 duplicates urn:ngsi-ld:OffStreetParking:v:1 (same name within 20 m)" in problems
    assert "urn:ngsi-ld:OffStreetParking:v:3 location lies outside praha +1 km" in problems
    assert "urn:ngsi-ld:OffStreetParking:v:3 availableSpotNumber 500 > totalSpotNumber 200" in problems
    assert "urn:ngsi-ld:OffStreetParking:v:4 totalSpotNumber -3 outside 0..inf" in problems
    assert "urn:ngsi-ld:OffStreetParking:v:4 location has lat/lon swapped" in problems
    assert "urn:ngsi-ld:OffStreetParking:v:5 location at 0,0" in problems


def test_a_type_without_a_vendored_model_is_no_sdm():
    assert board.sdm_check([{"id": "urn:ngsi-ld:Foo:1", "type": "Foo"}], "Foo", board.schemas()).startswith("ko: Foo is no")
    assert "Bridge" not in board.schemas() and "OffStreetParking" in board.schemas()
