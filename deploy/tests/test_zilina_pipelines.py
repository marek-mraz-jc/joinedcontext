"""Every Žilina mapping over a recorded answer of the real feed (T-3138, PL-03, PL-64).

The committed Bloblang runs in the pinned Bento over documents recorded on 2026-10-06 from the
URL each `DataSource` declares (fixtures/zilina), so a mapping checked here is a mapping that ran.
The empty-read guard every stream starts with is checked the same way: an answer with nothing in
it fails the run instead of writing nothing as if the city had emptied.
"""

import csv
import datetime
import http.server
import io
import json
import shutil
import subprocess
import threading
import typing
import urllib.parse
import zipfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-zilina-pipelines")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/zilina"
FIXTURES = Path(__file__).resolve().parent / "fixtures/zilina"
DOMAIN = "zilina.sk"
# What the one runner carries: the installation's organization, never the city (T-4202, T-4682).
RUNNER_DOMAIN = "hel.fi"
CITY = "SK031B517402"


def pipelines(prefix: str) -> dict[str, dict]:
    """Every pipeline manifest of the seed whose DataSource name starts with `prefix`."""
    found = {}
    for path in sorted(SEED.glob("zilina-pipeline-*.yaml")):
        if path.name.endswith("-bento.yaml"):
            continue
        manifest = yaml.safe_load(path.read_text())
        source = manifest["spec"]["source"].get("dataSourceRef", {}).get("name", "")
        if source.startswith(prefix):
            found[manifest["metadata"]["name"]] = {"source": source, "manifest": manifest,
                                                   "bento": path.with_name(path.stem + "-bento.yaml")}
    return found


CUBES = pipelines("susr-")
STATIONS = pipelines("eea-")


def run(pipeline: dict, space: str) -> list[dict]:
    suffix = ".parquet" if pipeline["source"].startswith("eea-") else ".json"
    document = (FIXTURES / (pipeline["source"] + suffix)).read_bytes()
    return open_data.run(pipeline["bento"], document, space, RUNNER_DOMAIN)


@pytest.fixture(scope="module")
def cubes():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(p, "zilina-mesto") for name, p in CUBES.items()}


@pytest.fixture(scope="module")
def stations():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(p, "zilina-verejne") for name, p in STATIONS.items()}


def test_every_inventoried_source_has_its_pipeline():
    assert len(CUBES) == 16 and len(STATIONS) == 5
    for p in [*CUBES.values(), *STATIONS.values()]:
        spec = p["manifest"]["spec"]
        assert spec["targetEndpoint"].startswith("urn:ngsi-ld:Endpoint:zilina.sk:")
        assert spec["schedule"], "every stream runs on a schedule"
        assert (SEED / f"zilina-datasource-{p['manifest']['metadata']['name']}.yaml").is_file()


def test_every_mapping_mints_the_citys_own_domain_never_the_runners():
    """The runner's `JC_ORG_DOMAIN` is hel.fi, so a mapping that read it minted Žilina's stops
    as `urn:ngsi-ld:GtfsStop:hel.fi:…` (T-4680 item 8, T-4682); the city's domain is a literal."""
    mappings = sorted(SEED.glob("zilina-pipeline-*-bento.yaml"))
    assert mappings
    for path in mappings:
        text = path.read_text()
        assert "JC_ORG_DOMAIN" not in text and "hel.fi" not in text, path.name
        if "urn:ngsi-ld:" in text:
            assert f'"{DOMAIN}"' in text, path.name


def test_every_stream_starts_with_the_empty_read_guard():
    for p in [*CUBES.values(), *STATIONS.values()]:
        assert "throw(" in p["bento"].read_text(), p["bento"].name


@requires_docker
def test_every_cube_cell_becomes_one_observation_of_the_city(cubes):
    schema = SEED / "statistical-observation.v1.schema.json"
    for name, entities in cubes.items():
        source = json.loads((FIXTURES / (CUBES[name]["source"] + ".json")).read_text())
        cells = [v for v in (source["value"] if isinstance(source["value"], list) else source["value"].values()) if v is not None]
        assert len(entities) == len(cells), name
        ids = [e["id"] for e in entities]
        assert len(set(ids)) == len(ids), f"{name}: two cells share an id"
        for entity in entities[:50]:
            assert entity["type"] == "KeyPerformanceIndicator"
            assert entity["id"].split(":")[3:5] == [DOMAIN, "zilina-mesto"]
            assert entity["refArea"]["value"] == CITY, name
            assert not open_data.schema_errors(schema, entity), (name, open_data.schema_errors(schema, entity))


@requires_docker
def test_the_population_of_the_city_is_the_one_ŠÚ_SR_publishes(cubes):
    end_of_quarter = {e["id"]: e for e in cubes["obyvatelstvo"]}[
        f"urn:ngsi-ld:KeyPerformanceIndicator:{DOMAIN}:zilina-mesto:om7101qr-{CITY}-2026Q2-IN010115-SPOLU"]
    assert end_of_quarter["kpiValue"]["value"] == 79617


@requires_docker
def test_the_station_is_one_entity_and_each_pollutant_its_own_reading(stations):
    expected = {
        "ovzdusie-pm10": ("pm10", 32.341, "GQ", "2026-10-06T18:00:00Z"),
        "ovzdusie-pm25": ("pm25", 15.417, "GQ", "2026-10-06T18:00:00Z"),
        "ovzdusie-no2": ("no2", 28.1313, "GQ", "2026-10-06T18:00:00Z"),
        # Every ozone hour of the recording carries validity 4, EEA's valid ozone reading.
        "ovzdusie-o3": ("o3", 11.8076, "GQ", "2026-10-05T17:00:00Z"),
        # The file's Unit column says mg.m-3: UN/CEFACT GP, never µg/m³.
        "ovzdusie-co": ("co", 0.63452, "GP", "2026-10-06T18:00:00Z"),
    }
    schema = SEED / "zilina-verejne.v1.schema.json"
    for name, (attribute, value, unit, at) in expected.items():
        [station] = stations[name]
        assert station["id"] == f"urn:ngsi-ld:AirQualityObserved:{DOMAIN}:zilina-verejne:eea-SK0020A"
        reading = station[attribute]
        assert (reading["value"], reading["unitCode"], reading["observedAt"]) == (value, unit, at), name
        assert station["location"]["value"] == {"type": "Point", "coordinates": [18.771215, 49.211447]}
        others = {"pm10", "pm25", "no2", "o3", "co"} - {attribute}
        assert not others & set(station), f"{name} writes only its own pollutant"
        assert not open_data.schema_errors(schema, station), open_data.schema_errors(schema, station)


def guard(path: Path) -> str:
    return next(step["mapping"] for step in yaml.safe_load(path.read_text())["pipeline"]["processors"]
                if "mapping" in step and "throw(" in step["mapping"])


def blobl(mapping: str, document: str) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", "run", "--rm", "-i", BENTO, "blobl", mapping],
                          input=document, capture_output=True, text=True, timeout=120)


@requires_docker
def test_an_empty_cube_answer_fails_the_run_and_a_full_one_passes():
    mapping = guard(CUBES["obyvatelstvo"]["bento"])
    empty = blobl(mapping, json.dumps({"value": [], "id": [], "size": []}) + "\n")
    assert "the cube answered no cell" in empty.stdout + empty.stderr
    refused = blobl(mapping, json.dumps({"status": 400, "status_message": "Bad number of dimension cubes"}) + "\n")
    assert "Bad number of dimension cubes" in refused.stdout + refused.stderr
    full = blobl(mapping, json.dumps({"value": [1]}) + "\n")
    assert '"value":[1]' in full.stdout.replace(" ", ""), full.stdout + full.stderr


@requires_docker
def test_an_empty_station_file_fails_the_run():
    mapping = guard(STATIONS["ovzdusie-pm10"]["bento"])
    empty = blobl(mapping, "[]\n")
    assert "the station file holds no record" in empty.stdout + empty.stderr


DREPO = SEED / "zilina-pipeline-drepo-bento.yaml"


class Recorded(http.server.BaseHTTPRequestHandler):
    """A feed's host answering from the recordings: DREPO's search API with its first three pages,
    cut to the fields the mapping reads and the page count set to three (the real library has
    fourteen), and the address register's file for Žilina, cut to the buildings the monuments name
    and forty others."""

    requested: typing.ClassVar[list[str]] = []

    def do_GET(self):
        Recorded.requested.append(self.path)
        url = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(url.query)
        if url.path.endswith("/discover/search/objects") and query.get("size") == ["100"]:
            body = FIXTURES / f"drepo-page{query.get('page', ['0'])[0]}.json"
        elif url.path == "/opendata/dataset/address_by_lau2_SK031B517402.geojson":
            body = FIXTURES / "mvsr-adresy-zilina.geojson"
        else:
            body = None
        if body is None or not body.is_file():
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body.read_bytes())

    def log_message(self, *_):
        pass


def offline(mapping: Path, host: str, space: str, document: bytes) -> subprocess.CompletedProcess:
    """`mapping` over `document`, its requests to `host` answered by `Recorded` on this machine."""
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Recorded)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = mapping.read_text().replace(host, f"http://127.0.0.1:{server.server_port}")
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": yaml.safe_load(text)["pipeline"],
            # The runner's shared egress limit (pipeline-runner/streams/resources.yaml).
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "all-bytes"}},
            "logger": {"level": "error"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", f"JC_ORG_DOMAIN={RUNNER_DOMAIN}",
             "-e", f"JC_SPACE={space}", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=document, capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def drepo(document: bytes) -> subprocess.CompletedProcess:
    return offline(DREPO, "https://dspace.uniza.sk", "zilina-uniza", document)


def recorded_items() -> list[dict]:
    return [o["_embedded"]["indexableObject"] for p in range(3)
            for o in json.loads((FIXTURES / f"drepo-page{p}.json").read_text())["_embedded"]["searchResult"]["_embedded"]["objects"]]


@requires_docker
def test_every_page_is_read_and_only_the_openly_licensed_works_enter():
    result = drepo((FIXTURES / "drepo-page0.json").read_bytes())
    assert result.returncode == 0, result.stderr.decode()
    works = json.loads(result.stdout)
    items = recorded_items()
    open_ = [i for i in items if i["metadata"].get("dc.rights.uri")]
    assert len(works) == len(open_) == 251, "every open item of all three pages, and nothing else"
    by_id = {w["id"]: w for w in works}
    assert len(by_id) == len(works)
    schema = SEED / "zilina-uniza.v1.schema.json"
    for work in works:
        assert work["id"].split(":")[3:5] == [DOMAIN, "zilina-uniza"]
        assert work["license"]["value"] == "http://creativecommons.org/licenses/by/4.0/"
        assert not open_data.schema_errors(schema, work), (work["id"], open_data.schema_errors(schema, work))
        assert not open_data.sdm_errors("extensions/Article", work), (work["id"], open_data.sdm_errors("extensions/Article", work))
        assert not {"author", "creator", "contributor"} & set(work)
    first = by_id[f"urn:ngsi-ld:Article:{DOMAIN}:zilina-uniza:hdluniza-936"]
    assert first["name"]["languageMap"] == {"sk": "Accuracy of digital terrain model on forest roads using airborne LIDAR and UAV point clouds"}
    assert (first["workType"]["value"], first["yearPublished"]["value"]) == ("Conference paper", 2023)
    assert first["url"]["value"] == "http://drepo.uniza.sk/handle/hdluniza/936"
    assert first["identifier"]["value"] == "hdluniza/936"
    # isPartOf is the collection's address (the Article extension's format uri), its name collectionName.
    # The harness serves DSpace from a local address, so only the path is fixed here.
    assert first["isPartOf"]["value"].endswith("/handle/hdluniza/933")
    assert first["collectionName"]["value"] == "Práce a štúdie - Vydanie 13"
    # No author name of any recorded item reaches the output, in any attribute.
    assert "Kardoš" not in result.stdout.decode()


@requires_docker
def test_a_title_of_no_stated_language_is_written_as_none():
    item = next(i for i in recorded_items() if i["metadata"].get("dc.rights.uri")
                and i["metadata"].get("dc.language.iso", [{}])[0].get("value") in (None, "other"))
    page = {"_embedded": {"searchResult": {"page": {"totalPages": 1, "totalElements": 1},
                                           "_embedded": {"objects": [{"_embedded": {"indexableObject": item}}]}}}}
    result = drepo(json.dumps(page).encode())
    [work] = json.loads(result.stdout)
    assert list(work["name"]["languageMap"]) == ["@none"]


@requires_docker
def test_a_closed_licence_and_a_withdrawn_item_stay_out():
    item = next(i for i in recorded_items() if i["metadata"].get("dc.rights.uri"))
    nc = json.loads(json.dumps(item))
    nc["metadata"]["dc.rights.uri"] = [{"value": "http://creativecommons.org/licenses/by-nc/4.0/"}]
    withdrawn = {**item, "withdrawn": True}
    sa = json.loads(json.dumps(item))
    sa["handle"] = "hdluniza/sa"
    sa["metadata"]["dc.rights.uri"] = [{"value": "https://creativecommons.org/licenses/by-sa/4.0/"}]
    objects = [{"_embedded": {"indexableObject": i}} for i in (nc, withdrawn, sa)]
    page = {"_embedded": {"searchResult": {"page": {"totalPages": 1, "totalElements": 3}, "_embedded": {"objects": objects}}}}
    result = drepo(json.dumps(page).encode())
    assert [w["id"].rsplit(":", 1)[1] for w in json.loads(result.stdout)] == ["hdluniza-sa"]


@requires_docker
def test_an_empty_library_and_a_missing_page_fail_the_run():
    empty = drepo(json.dumps({"_embedded": {"searchResult": {"page": {"totalPages": 0, "totalElements": 0}}}}).encode())
    assert b"the library answered no item" in empty.stdout + empty.stderr
    page = json.loads((FIXTURES / "drepo-page0.json").read_text())
    page["_embedded"]["searchResult"]["page"]["totalPages"] = 4  # page 3 answers 404
    missing = drepo(json.dumps(page).encode())
    assert b"page 3 did not come" in missing.stdout + missing.stderr


@requires_docker
def test_a_page_count_past_four_times_the_librarys_fails_the_run_with_no_request():
    """T-5701: DREPO had 14 pages on 2026-10-25, the cap is 56; a page 0 naming 100000 fetches nothing
    and writes none of its own items either."""
    page = json.loads((FIXTURES / "drepo-page0.json").read_text())
    page["_embedded"]["searchResult"]["page"]["totalPages"] = 100000
    Recorded.requested.clear()
    result = drepo(json.dumps(page).encode())
    assert b"the library names 100000 pages; more than 56" in result.stderr
    assert json.loads(result.stdout or b"[]") == []
    assert Recorded.requested == []
    page["_embedded"]["searchResult"]["page"]["totalPages"] = 3
    assert len(json.loads(drepo(json.dumps(page).encode()).stdout)) == 251


VLAKY = SEED / "zilina-pipeline-vlaky-bento.yaml"
GTFS = FIXTURES / "zsr-gtfs.zip"
CITY_STATIONS = {"Žilina", "Žilina-zariečie", "Žilina-Solinky", "Bytčica", "Brodno"}


def vlaky(document: bytes, day: str) -> subprocess.CompletedProcess:
    """The vlaky mapping over `document` as if it ran at noon of `day` in Žilina."""
    text = VLAKY.read_text()
    assert text.count("now()") == 1, "the mapping reads the clock once, which the test sets"
    text = text.replace("now()", f'"{day}T10:00:00Z".ts_parse("2006-01-02T15:04:05Z07:00")')
    return subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", f"JC_ORG_DOMAIN={RUNNER_DOMAIN}", "-e", "JC_SPACE=zilina-verejne",
         "-e", "CONFIG=" + json.dumps({"input": {"stdin": {"scanner": {"to_the_end": {}}}},
                                      "pipeline": yaml.safe_load(text)["pipeline"],
                                      "output": {"stdout": {"codec": "all-bytes"}}, "logger": {"level": "error"}}),
         "--entrypoint", "sh", BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=180,
    )


def departures(day: str) -> dict[str, int]:
    """The same count, written plainly over the recorded timetable: the oracle the mapping meets."""
    archive = zipfile.ZipFile(GTFS)
    table = lambda name: list(csv.DictReader(io.TextIOWrapper(archive.open(name), "utf-8-sig")))
    weekday = datetime.date(int(day[:4]), int(day[5:7]), int(day[8:])).strftime("%A").lower()
    date = day.replace("-", "")
    exceptions = [e for e in table("calendar_dates.txt") if e["date"] == date]
    services = {c["service_id"] for c in table("calendar.txt")
                if c[weekday] == "1" and c["start_date"] <= date <= c["end_date"]}
    services -= {e["service_id"] for e in exceptions if e["exception_type"] == "2"}
    services |= {e["service_id"] for e in exceptions if e["exception_type"] == "1"}
    running = {t["trip_id"] for t in table("trips.txt") if t["service_id"] in services}
    calls = [s for s in table("stop_times.txt") if s["trip_id"] in running]
    last = {}
    for s in calls:
        last[s["trip_id"]] = max(last.get(s["trip_id"], -1), int(s["stop_sequence"]))
    names = {s["stop_id"]: s["stop_name"] for s in table("stops.txt") if s["stop_name"] in CITY_STATIONS}
    out = dict.fromkeys(names.values(), 0)
    for s in calls:
        if s["stop_id"] in names and int(s["stop_sequence"]) < last[s["trip_id"]]:
            out[names[s["stop_id"]]] += 1
    return out


@requires_docker
@pytest.mark.parametrize("day", ["2026-10-07", "2026-10-11"])  # a Wednesday, a Sunday
def test_each_station_of_the_city_counts_the_trains_leaving_it_that_day(day):
    result = vlaky(GTFS.read_bytes(), day)
    assert result.returncode == 0, result.stderr.decode()
    stations = json.loads(result.stdout)
    assert {s["name"]["languageMap"]["sk"] for s in stations} == CITY_STATIONS
    counted = {s["name"]["languageMap"]["sk"]: s["dailyDepartures"]["value"] for s in stations}
    assert counted == departures(day)
    assert counted["Žilina"] > 100, "the main station of a four-line node"
    schema = SEED / "zilina-verejne.v1.schema.json"
    for station in stations:
        assert station["id"] == f"urn:ngsi-ld:GtfsStop:{DOMAIN}:zilina-verejne:zsr-{station['stopCode']['value']}"
        assert not open_data.schema_errors(schema, station), open_data.schema_errors(schema, station)


@requires_docker
def test_a_weekday_runs_more_trains_than_a_sunday():
    assert departures("2026-10-07")["Žilina"] > departures("2026-10-11")["Žilina"]


@requires_docker
def test_an_expired_timetable_and_a_zip_without_the_city_fail_the_run():
    expired = vlaky(GTFS.read_bytes(), "2027-01-15")
    assert b"the timetable runs no train from the city on 20270115" in expired.stdout + expired.stderr
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\n1,Košice,48.7,21.2\n")
    elsewhere = vlaky(buffer.getvalue(), "2026-10-07")
    assert b"the timetable names none of the city's stations" in elsewhere.stdout + elsewhere.stderr


@requires_docker
def test_a_timetable_zip_past_four_times_its_measured_size_writes_no_station():
    """T-5701: 290550 bytes on 2026-10-25, the cap four times that; the zip is refused before the unpack."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(GTFS) as recorded, zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
        for name in recorded.namelist():
            archive.writestr(name, recorded.read(name))
        archive.writestr("padding.txt", "x" * 1162200)
    result = vlaky(buffer.getvalue(), "2026-10-07")
    assert json.loads(result.stdout) == []
    assert b"more than 1162200 is not the national GTFS feed" in result.stderr


PAMIATKY = SEED / "zilina-pipeline-pamiatky-bento.yaml"
REGISTER = FIXTURES / "mksr-pamiatky-zsk.csv"


def pamiatky(document: bytes) -> subprocess.CompletedProcess:
    return offline(PAMIATKY, "https://rageo.minv.sk", "zilina-verejne", document)


@requires_docker
def test_every_object_of_a_city_monument_is_one_point_placed_at_its_building():
    result = pamiatky(REGISTER.read_bytes())
    assert result.returncode == 0, result.stderr.decode()
    points = json.loads(result.stdout)
    rows = [r for r in csv.DictReader(io.StringIO(REGISTER.read_text(encoding="utf-8"))) if r["Obec"] == "Žilina"]
    assert len(points) == len(rows) == 104, "one entity per object of the city, none from elsewhere"
    assert len({p["id"] for p in points}) == len(points)
    by_number = {p["monumentNumber"]["value"]: p for p in points}
    schema = SEED / "zilina-verejne.v1.schema.json"
    for point in points:
        assert not open_data.schema_errors(schema, point), (point["id"], open_data.schema_errors(schema, point))
    # Of the 85 objects with a súpisné číslo, 83 have their building in the register of addresses;
    # the other two name a number the register does not hold in that cadastral area.
    placed = [p for p in points if "location" in p]
    assert len(placed) == 83
    # A corner house is placed at the entrance the register names (Mariánske nám. 23, not Jezuitská 4).
    corner = next(p for p in placed if p["address"]["value"].startswith("Mariánske námestie 158/"))
    assert corner["address"]["value"] == "Mariánske námestie 158/23, Žilina"
    # A column in a square has no address and so no position; it is listed all the same.
    column = by_number["1317/2"]
    assert (column["monumentKind"]["value"], "location" in column) == ("SÚSOŠIE", False)
    assert column["name"]["languageMap"] == {"sk": "Trojičný stĺp"}


@requires_docker
def test_no_parcel_author_or_owner_leaves_the_register():
    result = pamiatky(REGISTER.read_bytes())
    points = json.loads(result.stdout)
    assert all(not {"author", "parcel", "parcelNumber", "owner"} & set(p) for p in points)
    authors = {r["Autor"] for r in csv.DictReader(io.StringIO(REGISTER.read_text(encoding="utf-8")))
               if r["Obec"] == "Žilina" and len(r["Autor"]) > 3}
    assert authors, "the recording names authors, so this check checks something"
    text = result.stdout.decode()
    assert not [a for a in authors if a in text]
    # Ownership is one of the register's categories ("Vlastníctvo cirkvi a cirk. organizácií"),
    # a dozen across the city, never a name.
    forms = {p["ownershipForm"]["value"] for p in points if "ownershipForm" in p}
    assert 3 <= len(forms) <= 15, forms


@requires_docker
def test_a_register_without_the_city_and_missing_addresses_fail_the_run():
    header = REGISTER.read_text(encoding="utf-8").splitlines()[0]
    elsewhere = pamiatky((header + "\r\n" + ",".join(["1", "Martin", "Martin"] + [""] * 16) + "\r\n").encode())
    assert b"the register holds no monument of the city" in elsewhere.stdout + elsewhere.stderr
    broken = offline(PAMIATKY, "https://rageo.minv.sk/opendata/dataset/address_by_lau2_", "zilina-verejne", REGISTER.read_bytes())
    assert b"the register of addresses did not come" in broken.stdout + broken.stderr


UKAZOVATELE = SEED / "zilina-pipeline-ukazovatele.yaml"
KPI_ENV = {"JC_ORG_DOMAIN": RUNNER_DOMAIN, "JC_SPACE": "zilina-kpi", "JC_SOURCE_SPACE": "zilina-mesto"}


def indicators(rows: list[dict]) -> list[dict]:
    from test_bystrica_indicators import bento
    mapping = yaml.safe_load(UKAZOVATELE.read_text())["spec"]["compute"]["bloblang"]
    return bento(mapping, rows, KPI_ENV, may_drop=True)


def queried(cubes: dict) -> list[dict]:
    """The rows of zilina-mesto the pipeline's query asks the Endpoint for."""
    q = yaml.safe_load(UKAZOVATELE.read_text())["spec"]["source"]["query"]["q"]
    sets, wanted = (part.split("==")[1].replace('"', "").split(",") for part in q.split(";"))
    return [e for rows in cubes.values() for e in rows if e["dataSet"]["value"] in sets and e["indicator"]["value"] in wanted]


@requires_docker
def test_the_six_indicators_are_the_cubes_newest_published_values(cubes):
    rows = queried(cubes)
    # About a thousand rows of the ~3 300 the five cubes hold; the endpoint read pages (T-3132).
    assert 900 < len(rows) < 1500, len(rows)
    by_name = {k["name"]["value"]: k for k in indicators(rows)}
    assert set(by_name) == {f"{n}-mesto" for n in (
        "obyvatelstvo-stav", "index-starnutia", "priemerny-vek", "celkovy-prirastok", "uchadzaci", "navstevnici-rok")}
    value = lambda name: (by_name[name]["currentValue"]["value"], by_name[name]["currentValue"]["unitCode"],
                          by_name[name]["calculationPeriod"]["value"])
    assert value("obyvatelstvo-stav-mesto") == (79617, "C62", {"start": "2026-04-01T00:00:00Z", "end": "2026-06-30T23:59:59Z"})
    year = lambda y: {"start": f"{y}-01-01T00:00:00Z", "end": f"{y}-12-31T23:59:59Z"}
    newest = lambda cube, indicator, key: max(
        (e for e in rows if (e["dataSet"]["value"], e["indicator"]["value"], e.get("dimensionKey", {}).get("value")) == (cube, indicator, key)),
        key=lambda e: e["refPeriod"]["value"])
    for name, cube, indicator, key, unit in (
        ("index-starnutia-mesto", "om7052rr", "IN010087", "SPOLU", "P1"),
        ("priemerny-vek-mesto", "om7052rr", "IN010089", "SPOLU", "ANN"),
        ("celkovy-prirastok-mesto", "om7105rr", "IN010082", "SPOLU", "C62"),
        ("uchadzaci-mesto", "pr5001rr", "U15061", None, "C62"),
    ):
        row = newest(cube, indicator, key)
        assert value(name) == (row["kpiValue"]["value"], unit, year(row["refPeriod"]["value"])), name
    # The visitors add up twelve months of the newest complete year, never 2026's first months.
    months = [e for e in rows if e["dataSet"]["value"] == "cr3803mr" and e["refPeriod"]["value"] == "2025"
              and e["dimensionKey"]["value"].endswith(".-VISIT_TOTAL")]
    assert len(months) == 12
    assert value("navstevnici-rok-mesto") == (sum(e["kpiValue"]["value"] for e in months), "C62", year("2025"))
    for kpi in by_name.values():
        assert kpi["id"] == f"urn:ngsi-ld:KeyPerformanceIndicator:{DOMAIN}:zilina-kpi:{kpi['name']['value']}"
        assert kpi["derivedFrom"]["value"] == f"urn:ngsi-ld:Endpoint:{DOMAIN}:zilina-mesto:zilina-mesto"
        assert kpi["currentValue"]["observedAt"].endswith("Z")
        # T-5330: the human name of Development/13 §2 in both languages, `name` being the slug.
        assert kpi["title"]["type"] == "LanguageProperty"
        assert set(kpi["title"]["languageMap"]) == {"sk", "en"}, kpi["id"]
    assert by_name["index-starnutia-mesto"]["title"]["languageMap"]["sk"] == "Index starnutia"


@requires_docker
def test_a_cube_that_drops_out_is_left_out_and_an_empty_read_writes_nothing(cubes):
    """T-4081: an indicator without a row is not written as "not measured" over a period that
    starts 0001-01-01; the value it had stays until the expiry removes it."""
    rows = [e for e in queried(cubes) if e["dataSet"]["value"] != "pr5001rr"]
    by_name = {k["name"]["value"]: k for k in indicators(rows)}
    assert "uchadzaci-mesto" not in by_name
    assert len(by_name) == 5
    assert by_name["obyvatelstvo-stav-mesto"]["currentValue"]["value"] == 79617
    for kpi in by_name.values():
        assert not kpi["calculationPeriod"]["value"]["start"].startswith("0001"), kpi["id"]
    assert indicators([]) == []
