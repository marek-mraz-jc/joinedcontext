"""Open Data BCN mobility into the space barcelona (T-5374): bike lanes, road accidents and the
traffic counters.

Each committed mapping runs in the pinned Bento over files recorded from
opendata-ajuntament.barcelona.cat on 2026-10-23 (tests/fixtures/barcelona, cut to a few dozen
records). The files a mapping reads itself come from a stand-in on this machine that answers by
path from the same recordings, or with a failure.
"""

import csv
import http.server
import json
import threading
import urllib.parse
from pathlib import Path
from typing import ClassVar

import open_data
import pytest
import yaml
from open_data import requires_docker
from test_barcelona_transport import bento

pytestmark = pytest.mark.xdist_group("docker-barcelona-open-data")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/barcelona"
FIXTURES = ROOT / "tests/fixtures/barcelona"
SCHEMA = SEED / "barcelona.v1.schema.json"
HOST = "https://opendata-ajuntament.barcelona.cat"
NOW = "2026-10-23T16:00:00Z"
BBOX = (2.04, 41.30, 2.24, 41.48)


class StandIn(http.server.BaseHTTPRequestHandler):
    """Answers each path from `files` (status, body); any other path is a 404."""

    files: ClassVar[dict[str, tuple[int, bytes]]] = {}
    seen: ClassVar[list[str]] = []

    def do_GET(self):
        StandIn.seen.append(self.path)
        missing = (404, b"not found")
        status, body = StandIn.files.get(self.path) or StandIn.files.get(self.path.split("?")[0], missing)
        self.send_response(status)
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def run(bento_file: str, document: bytes, files: dict[str, tuple[int, bytes]], now: str = NOW,
        counted: bool = False) -> tuple[list[dict], list[str]]:
    """The mapping `bento_file` over `document`, its own reads answered by the stand-in from
    `files` (path -> status, body; a body may name the stand-in as {base}). With `counted`, the
    message the run logs instead of the entities."""
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StandIn)
    base = f"http://localhost:{server.server_port}"
    StandIn.files = {path: (status, body.replace(b"{base}", base.encode())) for path, (status, body) in files.items()}
    StandIn.seen = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (SEED / bento_file).read_text().replace(HOST, base)
        text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
        processors = yaml.safe_load(text)["pipeline"]["processors"]
        if counted:
            steps = processors[0]["try"]
            processors = [{"try": steps[:steps.index(next(s for s in steps if "log" in s))]}]
        return bento(processors, document.replace(b"{base}", base.encode()))
    finally:
        server.shutdown()


def inside(point: list[float]) -> bool:
    return BBOX[0] <= point[0] <= BBOX[2] and BBOX[1] <= point[1] <= BBOX[3]


# Bike lanes ------------------------------------------------------------------------------------

LANE = "urn:ngsi-ld:BikeLane:barcelona.cat:barcelona:carril-{}"
CONSTRUCTION = "/resources/bcn/CarrilsBici/CARRIL_BICI_CONSTRUCCIO.geojson"
CORRIDORS = "/resources/bcn/CarrilsBici/CORREDORS_BICI.geojson"


def layer(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text())


def lanes(open_layer: dict | None = None, construction: bytes | None = None, status: int = 200,
          counted: bool = False, corridors: bytes | None = None, corridor_status: int = 200) -> tuple[list[dict], list[str]]:
    document = json.dumps(layer("carril-bici.geojson") if open_layer is None else open_layer).encode()
    body = (FIXTURES / "carril-bici-construccio.geojson").read_bytes() if construction is None else construction
    corridor_body = (FIXTURES / "corredors-bici.geojson").read_bytes() if corridors is None else corridors
    return run("barcelona-pipeline-bike-lanes-bento.yaml", document,
               {CONSTRUCTION: (status, body), CORRIDORS: (corridor_status, corridor_body)}, counted=counted)


@requires_docker
def test_every_recorded_segment_is_a_valid_bike_lane_open_corridor_or_under_construction():
    written, errors = lanes()
    assert errors == []
    by_id = {e["id"]: e for e in written}
    for name, state, dataset in (("carril-bici.geojson", "inService", "carril-bici"),
                                 ("corredors-bici.geojson", "inService", "corredors-bici-bcn"),
                                 ("carril-bici-construccio.geojson", "underConstruction", "carrils-bici-en-construccio")):
        for feature in layer(name)["features"]:
            entity = by_id[LANE.format(feature["properties"]["ID"].lower())]
            assert not open_data.schema_errors(SCHEMA, entity), entity
            assert not open_data.sdm_errors("dataModel.Transportation/BikeLane", entity), entity
            assert entity["bikeLaneState"]["value"] == state
            assert entity["location"]["value"] == feature["geometry"]
            assert entity["name"]["value"] == feature["properties"]["TOOLTIP"].strip()
            assert entity["localId"]["value"] == feature["properties"]["ID"]
            assert 0 < entity["laneLength"]["value"] < 20000 and entity["laneLength"]["unitCode"] == "MTR"
            assert entity["expiresAt"] == "2026-11-13T16:00:00Z"
            assert entity["source"]["value"].endswith(f"/data/dataset/{dataset}")
    assert len(written) == 32
    # GL4683814 runs two points 56.7 m apart on Independència (haversine).
    assert by_id[LANE.format("gl4683814")]["laneLength"]["value"] == 57


@requires_docker
def test_an_out_of_area_line_a_malformed_line_and_a_duplicate_segment_are_left_out_and_counted():
    recorded = layer("carril-bici.geojson")
    features = recorded["features"]
    swapped = json.loads(json.dumps(features[0]))
    swapped["properties"]["ID"] = "GL9000001"
    swapped["geometry"]["coordinates"] = [[lat, lon] for lon, lat in swapped["geometry"]["coordinates"]]
    single = json.loads(json.dumps(features[1]))
    single["properties"]["ID"] = "GL9000002"
    single["geometry"]["coordinates"] = single["geometry"]["coordinates"][:1]
    uncoded = json.loads(json.dumps(features[2]))
    uncoded["properties"]["ID"] = "carril"
    again = json.loads(json.dumps(features[3]))
    again["properties"]["TOOLTIP"] = "repeated"
    recorded["features"] = [*features, swapped, single, uncoded, again]
    # The construction layer lists an open lane too: the open state stands.
    construction = layer("carril-bici-construccio.geojson")
    construction["features"].append(json.loads(json.dumps(features[4])))
    out, errors = lanes(recorded, json.dumps(construction).encode(), counted=True)
    assert errors == []
    assert out[0]["dropped"] == {"noSegmentCode": 1, "lineMalformed": 1, "outsideTheCity": 1, "duplicateSegment": 2,
                                 "lengthOutOfRange": 0}
    by_id = {lane["id"]: lane for lane in out[0]["lanes"]}
    assert len(by_id) == len(out[0]["lanes"]) == 32
    assert by_id[features[3]["properties"]["ID"]]["name"] == features[3]["properties"]["TOOLTIP"].strip()
    assert by_id[features[4]["properties"]["ID"]]["state"] == "inService"
    assert all(inside(p) for lane in out[0]["lanes"] for p in lane["line"])


@pytest.mark.parametrize("open_layer, construction, status, corridors, corridor_status, reason", [
    ({"type": "FeatureCollection", "features": []}, None, 200, None, 200, "holds no open lane"),
    ({"html": "302 Found"}, None, 200, None, 200, "no GeoJSON FeatureCollection"),
    (None, b"<html>302 Found</html>", 200, None, 200, "construction layer is no GeoJSON"),
    (None, None, 503, None, 200, ""),
    (None, b" " * 1048577, 200, None, 200, "more than the 1 MiB read"),
    (None, None, 200, b"<html>302 Found</html>", 200, "corridor layer is no GeoJSON"),
    (None, None, 200, None, 503, ""),
], ids=["empty", "not-geojson", "construction-html", "construction-503", "construction-oversize", "corridor-html",
        "corridor-503"])
@requires_docker
def test_an_empty_or_broken_bike_lane_layer_fails_the_run_and_writes_nothing(open_layer, construction, status, corridors,
                                                                              corridor_status, reason):
    written, errors = lanes(open_layer, construction, status, corridors=corridors, corridor_status=corridor_status)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


@requires_docker
def test_an_oversized_bike_lane_layer_fails_the_run():
    written, errors = run("barcelona-pipeline-bike-lanes-bento.yaml", b" " * 4194305, {})
    assert written == [] and len(errors) == 1 and "more than the 4 MiB read" in errors[0]


# Road accidents --------------------------------------------------------------------------------

ACCIDENT = "urn:ngsi-ld:RoadAccident:barcelona.cat:barcelona:accident-{}"
ACCIDENTS_CSV = "/data/dataset/e769eb9d-d778-4cd7-9e3a-5858bba49b20/resource/066d46b1-25be-4f08-b0e0-5a233714bda2/download/2025_accidents_gu_bcn.csv"


def accident_rows(text: bytes | None = None) -> list[dict]:
    text = (FIXTURES / "2025_accidents_gu_bcn.csv").read_bytes() if text is None else text
    return list(csv.DictReader(text.decode("utf-8-sig").splitlines()))


DETAILS_SEARCH = "/data/api/3/action/package_search"
DETAILS_SQL = "/data/api/3/action/datastore_search_sql"


def accidents(csv_body: bytes | None = None, record: bytes | None = None, status: int = 200, now: str = NOW,
              counted: bool = False, search: bytes | None = None, details: bytes | None = None,
              details_status: int = 200) -> tuple[list[dict], list[str]]:
    record = (FIXTURES / "package-accidents-gu-bcn.json").read_bytes() if record is None else record
    body = (FIXTURES / "2025_accidents_gu_bcn.csv").read_bytes() if csv_body is None else csv_body
    search = (FIXTURES / "package-search-accident-details.json").read_bytes() if search is None else search
    details = (FIXTURES / "accident-details-sql.json").read_bytes() if details is None else details
    return run("barcelona-pipeline-accidents-bento.yaml", record.replace(HOST.encode(), b"{base}"),
               {ACCIDENTS_CSV: (status, body), DETAILS_SEARCH: (200, search), DETAILS_SQL: (details_status, details)},
               now=now, counted=counted)


@requires_docker
def test_every_recorded_accident_is_a_valid_road_accident_with_counts_only():
    written, errors = accidents()
    assert errors == []
    by_id = {e["id"]: e for e in written}
    rows = accident_rows()
    assert len(written) == len(rows) == 34
    for row in rows:
        entity = by_id[ACCIDENT.format(row["Numero_expedient"].lower())]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        assert not open_data.sdm_errors("dataModel.Transportation/RoadAccident", entity), entity
        slight, serious, dead = (int(row[c] or 0) for c in ("Numero_lesionats_lleus", "Numero_lesionats_greus", "Numero_morts"))
        assert entity["totalInjured"]["value"] == slight + serious
        assert (entity["slightlyInjured"]["value"], entity["seriouslyInjured"]["value"], entity["deadCount"]["value"]) == (slight, serious, dead)
        assert entity["vehicleCount"]["value"] == int(row["Numero_vehicles_implicats"])
        assert entity["location"]["value"]["coordinates"] == [float(row["Longitud_WGS84"]), float(row["Latitud_WGS84"])]
        assert entity["areaServed"]["value"] == row["Nom_barri"]
        assert ("pedestrianCause" in entity) == ("pedestriansInvolved" in entity) == (row["Descripcio_causa_vianant"] != "")
    # 2025S000001: 1 January 2025, 04:00 in Barcelona (CET), 03:00 UTC, Migdia 187.
    first = by_id[ACCIDENT.format("2025s000001")]
    assert first["accidentDate"]["value"] == "2025-01-01T03:00:00Z"
    assert first["address"]["value"] == {"streetAddress": "Migdia 187", "addressLocality": "Barcelona",
                                         "district": "Sants-Montjuïc", "addressCountry": "ES"}
    assert sum(e["deadCount"]["value"] for e in written) == 2
    # The detail tables join by file number: kinds, causes (mediate, then the drivers'), one kind
    # per vehicle, and passengers and pedestrians as counts.
    details = {r["id"]: r for r in json.loads((FIXTURES / "accident-details-sql.json").read_text())["result"]["records"]}
    for entity in written:
        row = details[entity["localId"]["value"]]
        assert entity["accidentKind"]["value"] == row["types"].split("|")
        assert entity["vehicleKind"]["value"] == row["vehicles"].split("|")
        assert entity["vehicleKind"]["value"], entity["id"]
        assert entity["numPedestrianInjured"]["value"] == int(row["wi"])
        assert entity["numPassengersDead"]["value"] == int(row["pd"])
    second = by_id[ACCIDENT.format("2025s000002")]
    assert second["accidentCause"]["value"] == ["Alcoholèmia", "Gir indegut o sense precaució"]
    assert second["vehicleKind"]["value"] == ["Motocicleta", "Turisme"]
    # SDM codes only where a code means the same: motorcycle 14, car 5; side collision 3.
    assert second["vehiclesInvolved"]["value"] == ["14", "5"] and second["accidentType"]["value"] == ["3"]
    first = by_id[ACCIDENT.format("2025s000001")]
    assert first["accidentCause"]["value"] == ["Manca d'atenció a la conducció"]
    assert first["accidentType"]["value"] == ["8"]
    # No field names or describes a person beyond a count.
    assert {k for e in written for k in e} <= {
        "id", "type", "localId", "status", "accidentDate", "totalInjured", "seriouslyInjured", "slightlyInjured",
        "deadCount", "vehicleCount", "pedestriansInvolved", "pedestrianCause", "accidentKind", "accidentType",
        "accidentCause", "vehicleKind", "vehiclesInvolved", "numPassengersInjured", "numPassengersDead", "numPedestrianInjured", "numPedestrianDead",
        "address", "areaServed", "location", "dataProvider", "source"}
    # One query over the five 2025 detail tables, ids checked first, read only.
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(StandIn.seen[-1]).query)["sql"][0]
    for resource in ("e7363470-3b6f-46d6-8cdc-57dabb69c6a6", "c939e28f-9662-4d71-b375-658e47dbb96b",
                     "796504f6-7602-41a7-82a9-d3bd47e68dee", "7921ef46-5a4e-4f77-8c91-9d315664a6e4",
                     "8e7388ee-ffbd-446e-a8ad-c427a99da48d"):
        assert f'"{resource}"' in query
    assert query.startswith("SELECT ") and ";" not in query


@requires_docker
def test_an_out_of_area_point_a_count_out_of_range_and_a_duplicate_file_are_left_out_and_counted():
    text = (FIXTURES / "2025_accidents_gu_bcn.csv").read_bytes().decode("utf-8")
    lines = text.rstrip("\r\n").split("\r\n")
    row = lines[1].split(",")
    swapped = [*row]
    swapped[0], swapped[-2], swapped[-1] = "2025S900001", row[-1], row[-2]
    negative = [*row]
    negative[0], negative[18] = "2025S900002", "-1"
    future = [*row]
    future[0], future[9] = "2025S900003", "2027"
    nodate = [*row]
    nodate[0], nodate[10] = "2025S900004", "13"
    unnumbered = [*row]
    unnumbered[0] = "expedient"
    extra = [",".join(r) for r in (swapped, negative, future, nodate, unnumbered)] + [lines[2]]
    out, errors = accidents("\r\n".join(lines + extra).encode() + b"\r\n", counted=True)
    assert errors == []
    assert out[0]["dropped"] == {"noFileNumber": 1, "noDate": 1, "noPoint": 0, "outsideTheCity": 1, "countOutOfRange": 1,
                                 "dateInFuture": 1, "duplicateFileNumber": 1}
    ids = [a["id"] for a in out[0]["accidents"]]
    assert len(ids) == len(set(ids)) == 34


@pytest.mark.parametrize("csv_body, record, status, now, reason", [
    (b"\xef\xbb\xbfNumero_expedient,Codi_districte\r\n", None, 200, NOW, "holds no accident"),
    (b"<html>302 Found</html>", None, 200, NOW, "holds no accident"),
    (None, None, 503, NOW, ""),
    (b" " * 4194305, None, 200, NOW, "more than the 4 MiB read"),
    (None, b'{"result": {"resources": []}}', 200, NOW, "names no YYYY_accidents_gu_bcn.csv"),
    (None, b"<html>busy</html>", 200, NOW, "names no YYYY_accidents_gu_bcn.csv"),
    (None, None, 200, "2028-01-02T00:00:00Z", "more than two years old"),
    (None, (FIXTURES / "package-accidents-gu-bcn.json").read_bytes().replace(
        f"{HOST}/data/dataset/".encode(), b"https://example.org/data/dataset/", 1), 200, NOW, "not on the city's dataset path"),
    (None, (FIXTURES / "package-accidents-gu-bcn.json").read_bytes().replace(
        b"/resource/066d46b1-25be-4f08-b0e0-5a233714bda2/", b"/../../api/3/action/066d46b1-25be-4f08-b0e0-5a233714bda2/"),
     200, NOW, "not on the city's dataset path"),
], ids=["empty", "html", "csv-503", "oversize", "no-files", "record-html", "stale", "foreign-host", "path-escape"])
@requires_docker
def test_an_empty_stale_or_broken_accident_source_fails_the_run_and_writes_nothing(csv_body, record, status, now, reason):
    written, errors = accidents(csv_body, record, status, now)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


@pytest.mark.parametrize("search, details, details_status, reason", [
    (b'{"result": {"results": []}}', None, 200, "are not all in the DataStore: accidents-tipus-gu-bcn"),
    ((FIXTURES / "package-search-accident-details.json").read_bytes().replace(
        b"8e7388ee-ffbd-446e-a8ad-c427a99da48d", b"8e7388ee select 1"), None, 200, "accidents-persones-gu-bcn"),
    (None, b'{"success": false, "error": {"message": "relation does not exist"}}', 200, "refused the accident details query"),
    (None, b"<html>Server Error</html>", 200, "refused the accident details query"),
    (None, None, 500, ""),
], ids=["no-tables", "id-not-uuid", "refused", "html", "sql-500"])
@requires_docker
def test_a_missing_or_broken_accident_detail_table_fails_the_run_and_writes_nothing(search, details, details_status, reason):
    written, errors = accidents(search=search, details=details, details_status=details_status)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


# Traffic counters ------------------------------------------------------------------------------

COUNTER = "urn:ngsi-ld:TrafficFlowObserved:barcelona.cat:barcelona:aforament-{}"
REGISTER = "/data/api/3/action/package_show?id=aforaments-descriptiu"
READINGS_SQL = "/data/api/3/action/datastore_search_sql"
READINGS_2025 = "45fc3faa-cdc6-4ac3-83e9-d019c8a629b9"
REGISTER_2025 = "36d41387-a4e5-48fa-801d-698257e3a106"


def counter_rows() -> list[dict]:
    return json.loads((FIXTURES / "aforaments-sql.json").read_text())["result"]["records"]


def counters(rows: list[dict] | None = None, record: bytes | None = None, register: bytes | None = None,
             answer: bytes | None = None, status: int = 200, now: str = NOW,
             counted: bool = False) -> tuple[list[dict], list[str]]:
    record = (FIXTURES / "package-aforaments-detall.json").read_bytes() if record is None else record
    register = (FIXTURES / "package-aforaments-descriptiu.json").read_bytes() if register is None else register
    if answer is None:
        doc = json.loads((FIXTURES / "aforaments-sql.json").read_text())
        if rows is not None:
            doc["result"]["records"] = rows
        answer = json.dumps(doc).encode()
    return run("barcelona-pipeline-traffic-counters-bento.yaml", record,
               {REGISTER: (200, register), READINGS_SQL: (status, answer)}, now=now, counted=counted)


@requires_docker
def test_every_recorded_counter_is_a_valid_traffic_flow_with_the_newest_working_day_intensity():
    written, errors = counters()
    assert errors == []
    by_id = {e["id"]: e for e in written}
    measured = [r for r in counter_rows() if r["imd"].isdigit()]
    assert len(written) == len(measured) == 20
    for row in measured:
        entity = by_id[COUNTER.format(row["id"].lower())]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        assert not open_data.sdm_errors("dataModel.Transportation/TrafficFlowObserved", entity), entity
        assert entity["intensity"]["value"] == int(row["imd"])
        assert entity["name"]["value"] == row["name"].strip()
        assert entity["location"]["value"]["coordinates"] == [float(row["lon"]), float(row["lat"])]
        assert ("vehicleType" in entity) == (row["kind"] == "Bicis")
        # December 2025 in Barcelona: from 1 December 00:00 CET to 1 January 00:00 CET.
        assert entity["dateObservedFrom"]["value"] == "2025-11-30T23:00:00Z"
        assert entity["dateObservedTo"]["value"] == entity["dateObserved"]["value"] == "2025-12-31T23:00:00Z"
    assert {e["vehicleType"]["value"] for e in written if "vehicleType" in e} == {"bicycle"}
    # One query over the newest year's readings joined to that year's register, ids checked first.
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(StandIn.seen[-1]).query)["sql"][0]
    assert f'FROM "{READINGS_2025}" v LEFT JOIN "{REGISTER_2025}" d' in query
    assert query.startswith("SELECT ") and ";" not in query


@requires_docker
def test_an_out_of_area_counter_an_intensity_out_of_range_and_a_duplicate_are_left_out_and_counted():
    rows = counter_rows()
    measured = [r for r in rows if r["imd"].isdigit()]
    swapped = dict(measured[0], id="99001", lon=measured[0]["lat"], lat=measured[0]["lon"])
    huge = dict(measured[1], id="99002", imd="250000")
    unplaced = dict(measured[2], id="99003", lon=None, lat=None)
    unnumbered = dict(measured[3], id="")
    out, errors = counters([*rows, swapped, huge, unplaced, unnumbered, measured[4]], counted=True)
    assert errors == []
    assert out[0]["dropped"] == {"noCounterId": 1, "noMeasurement": 3, "noPlace": 1, "outsideTheCity": 1,
                                 "intensityOutOfRange": 1, "duplicateCounter": 1}
    ids = [c["id"] for c in out[0]["counters"]]
    assert len(ids) == len(set(ids)) == 20
    assert all(inside([c["lon"], c["lat"]]) and 0 <= c["imd"] <= 200000 for c in out[0]["counters"])


@pytest.mark.parametrize("rows, record, register, answer, status, now, reason", [
    ([], None, None, None, 200, NOW, "hold no counter"),
    (None, None, None, b'{"success": false, "error": {"message": "relation does not exist"}}', 200, NOW,
     "refused the counter readings query"),
    (None, None, None, b"<html>Server Error</html>", 200, NOW, "refused the counter readings query"),
    (None, None, None, None, 500, NOW, ""),
    (None, None, None, b" " * 4194305, 200, NOW, "more than the 4 MiB read"),
    (None, b'{"result": {"resources": []}}', None, None, 200, NOW, "names no YYYY_aforament_detall_valor.csv"),
    (None, None, b'{"result": {"resources": []}}', None, 200, NOW, "names no YYYY_aforament_descripcio.csv"),
    (None, (FIXTURES / "package-aforaments-detall.json").read_bytes().replace(
        READINGS_2025.encode(), b"45fc3faa select 1"), None, None, 200, "2026-01-01T00:00:00Z",
     "of 2024"),
    (None, None, None, None, 200, "2028-01-02T00:00:00Z", "more than two years old"),
], ids=["empty", "refused", "html", "sql-500", "oversize", "no-readings", "no-register", "id-not-uuid", "stale"])
@requires_docker
def test_an_empty_stale_or_broken_counter_source_fails_the_run_and_writes_nothing(rows, record, register, answer, status,
                                                                                  now, reason):
    written, errors = counters(rows, record, register, answer, status, now)
    if reason == "of 2024":
        # A resource id that is no UUID is never read: the run falls back to the year before.
        assert errors == [] and READINGS_2025 not in StandIn.seen[-1]
        return
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors
