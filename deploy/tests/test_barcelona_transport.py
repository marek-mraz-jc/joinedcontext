"""Open Data BCN transport into the space barcelona (T-5262).

Each committed mapping runs in the pinned Bento over files recorded from
opendata-ajuntament.barcelona.cat on 2026-10-22 (tests/fixtures/barcelona, cut to a few dozen
records, line endings as published). The section list the traffic pipeline reads itself comes
from a stand-in on this machine that answers from the same recording, or with a failure.
"""

import http.server
import json
import subprocess
import threading
from pathlib import Path
from typing import ClassVar

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-open-data")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/barcelona"
FIXTURES = ROOT / "tests/fixtures/barcelona"
RESOURCES = ROOT / "components/pipeline-runner/streams/resources.yaml"
SCHEMA = SEED / "barcelona.v1.schema.json"
HOST = "https://opendata-ajuntament.barcelona.cat"
# The state file was written at 22:10:56 Barcelona time, 20:10:56 UTC.
NOW = "2026-10-22T20:15:00Z"
URN = "urn:ngsi-ld:TrafficFlowObserved:barcelona.cat:barcelona:tram-{}"


class OpenDataBcn(http.server.BaseHTTPRequestHandler):
    """The section list as recorded, or `status` instead of it."""

    status: ClassVar[int] = 200
    body: ClassVar[bytes] = b""

    def do_GET(self):
        self.send_response(OpenDataBcn.status)
        self.send_header("Content-Type", "text/csv")
        self.end_headers()
        self.wfile.write(OpenDataBcn.body)

    def log_message(self, *_):
        pass


def bento(processors: list, document: bytes) -> tuple[list[dict], list[str]]:
    processors = [*processors, {"mapping": 'root = if errored() { {"__error": error()} } else { this }'}]
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": {"processors": processors},
        "rate_limit_resources": yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"],
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "none"},
    }
    done = subprocess.run(
        ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=barcelona.cat", "-e", "JC_SPACE=barcelona",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=180, check=False,
    )
    assert done.returncode == 0, done.stderr.decode()
    entities, errors = [], []
    for line in done.stdout.decode().splitlines():
        if line.strip():
            message = json.loads(line)
            if isinstance(message, dict) and "__error" in message:
                errors.append(message["__error"])
            else:
                entities.append(message)
    return entities, errors


def traffic(document: bytes, sections: bytes | None = None, status: int = 200, now: str = NOW,
            counted: bool = False) -> tuple[list[dict], list[str]]:
    """The traffic-state mapping over `document` with its clock at `now`. With `counted`, the
    message the run logs (entities and the dropped counts) instead of the entities."""
    OpenDataBcn.status = status
    OpenDataBcn.body = (FIXTURES / "transit_relacio_trams.csv").read_bytes() if sections is None else sections
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), OpenDataBcn)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (SEED / "barcelona-pipeline-traffic-state-bento.yaml").read_text()
        text = text.replace(f"{HOST}/data/", f"http://127.0.0.1:{server.server_port}/data/")
        text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
        processors = yaml.safe_load(text)["pipeline"]["processors"]
        if counted:
            steps = processors[0]["try"]
            processors = [{"try": steps[:steps.index(next(s for s in steps if "log" in s))]}]
        return bento(processors, document)
    finally:
        server.shutdown()


def recorded() -> bytes:
    return (FIXTURES / "traffic-state.dat").read_bytes()


def rows() -> list[list[str]]:
    return [line.strip().split("#") for line in recorded().decode().splitlines() if line.strip()]


@requires_docker
def test_every_recorded_section_with_a_line_is_a_valid_traffic_flow_observed():
    written, errors = traffic(recorded())
    assert errors == []
    # 36 lines, section 535 has no line in the section list (the city lists 526 of the 532).
    assert len(written) == 35
    codes = ["noData", "veryFluid", "fluid", "dense", "veryDense", "congestion", "closed"]
    by_id = {e["id"]: e for e in written}
    for section, _at, state, forecast in rows():
        if section == "535":
            assert URN.format(section) not in by_id
            continue
        entity = by_id[URN.format(section)]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        assert entity["trafficState"]["value"] == codes[int(state)]
        assert entity["trafficStateForecast"]["value"] == codes[int(forecast)]
        assert entity["congested"]["value"] is (state in ("4", "5"))
        assert entity["dateObserved"]["value"] == "2026-10-22T20:10:56Z"
        assert entity["trafficState"]["observedAt"] == "2026-10-22T20:10:56Z"
        line = entity["location"]["value"]
        assert line["type"] == "LineString" and len(line["coordinates"]) >= 2
        assert all(2.04 <= lon <= 2.24 and 41.30 <= lat <= 41.48 for lon, lat in line["coordinates"])
    assert {e["trafficState"]["value"] for e in written} == set(codes)
    first = by_id[URN.format("1")]
    assert first["name"]["value"] == "Diagonal (Ronda de Dalt a Doctor Marañón)"
    assert first["location"]["value"]["coordinates"][0] == [2.11203535639414, 41.3841912394771]


@requires_docker
def test_an_out_of_area_line_a_code_out_of_range_and_a_duplicate_are_left_out_and_counted():
    sections = (FIXTURES / "transit_relacio_trams.csv").read_bytes().replace(
        b'"2.11203535639414,41.3841912394771,', b'"41.3841912394771,2.11203535639414,')  # section 1, lat/lon swapped
    lines = recorded().decode().splitlines()
    extra = ["2#20261022221056#9#1", "3#20261022221056#2#2", "x#y", "4#20261025221056#2#2"]
    document = "\r\n".join(lines + extra).encode()
    out, errors = traffic(document, sections, counted=True)
    assert errors == []
    assert out[0]["dropped"] == {
        "malformedLine": 1, "stateOutOfRange": 1, "timeInFuture": 1, "duplicateSection": 1,
        "sectionWithoutLine": 2, "lineMalformed": 0, "lineOutOfArea": 1,
    }
    ids = [e["id"] for e in out[0]["entities"]]
    assert URN.format("1") not in ids and len(ids) == len(set(ids)) == 34
    # The duplicate 3 is the second line: the first, as recorded, is kept.
    three = next(e for e in out[0]["entities"] if e["id"] == URN.format("3"))
    assert three["trafficState"]["value"] == ["noData", "veryFluid", "fluid", "dense", "veryDense", "congestion", "closed"][
        int(next(r for r in rows() if r[0] == "3")[2])]


@pytest.mark.parametrize("document, sections, status, now, reason", [
    (b"", None, 200, NOW, "names no section"),
    (b"<html>maintenance</html>", None, 200, NOW, "names no section"),
    (None, None, 200, "2026-10-22T20:30:00Z", "older than three 5-minute periods"),
    (None, None, 503, NOW, ""),
    (None, b"id;name\r\n1;x\r\n", 200, NOW, "Tram, Descripció, Coordenades"),
    (b" " * 262145, None, 200, NOW, "more than the 256 kB read"),
], ids=["empty", "html", "stale", "sections-503", "sections-columns", "oversize"])
@requires_docker
def test_an_empty_stale_or_broken_source_fails_the_run_and_writes_nothing(document, sections, status, now, reason):
    written, errors = traffic(recorded() if document is None else document, sections, status, now)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


def test_the_seed_reads_open_data_bcn_every_five_minutes_and_grants_only_the_added_types():
    source = yaml.safe_load((SEED / "barcelona-datasource-traffic-state.yaml").read_text())["spec"]["http"]
    assert source["url"].startswith(f"{HOST}/data/dataset/")
    pipeline = yaml.safe_load((SEED / "barcelona-pipeline-traffic-state.yaml").read_text())["spec"]
    assert pipeline["period"] == "300s"
    assert pipeline["output"] == {"type": "TrafficFlowObserved", "mode": "upsert"}
    # Every call the mapping makes itself goes to the same host.
    text = (SEED / "barcelona-pipeline-traffic-state-bento.yaml").read_text()
    urls = [line.split("url:", 1)[1].strip() for line in text.splitlines() if line.strip().startswith("url:")]
    assert urls and all(u.startswith(f"{HOST}/data/dataset/") for u in urls)
    write = yaml.safe_load((SEED / "barcelona-policy-pipelines-write.yaml").read_text())["spec"]
    assert "deleteBatch" not in write["operations"]
    granted = {e["type"] for rule in write["information"] for e in rule["entities"]}
    model = yaml.safe_load((SEED / "barcelona-datamodel.yaml").read_text())["spec"]["classes"]
    assert granted == set(model) >= {"TrafficFlowObserved"}
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    for name in ("datasource-traffic-state", "pipeline-traffic-state", "pipeline-traffic-state-bento",
                 "endpoint-traffic", "policy-public-traffic"):
        assert f"barcelona-{name}.yaml" in index


PARKING = "urn:ngsi-ld:OnStreetParking:barcelona.cat:barcelona:bsm-{}-{}"


def parking(document: bytes, counted: bool = False) -> tuple[list[dict], list[str]]:
    processors = yaml.safe_load((SEED / "barcelona-pipeline-parking-sections-bento.yaml").read_text())["pipeline"]["processors"]
    if counted:
        steps = processors[0]["try"]
        processors = [{"try": steps[:steps.index(next(s for s in steps if "log" in s))]}]
    return bento(processors, document)


def pieces() -> list[dict]:
    text = (FIXTURES / "TRAMS.CSV").read_bytes().decode("utf-8-sig")
    return list(__import__("csv").DictReader(text.splitlines()))


@requires_docker
def test_every_recorded_parking_section_is_one_valid_on_street_parking_with_its_pieces():
    written, errors = parking((FIXTURES / "TRAMS.CSV").read_bytes())
    assert errors == []
    sections: dict[tuple[str, str], list[dict]] = {}
    for row in pieces():
        sections.setdefault((row["ID_TRAM"], row["TIPUS_TRAM"]), []).append(row)
    assert len(written) == len(sections) == 15
    by_id = {e["id"]: e for e in written}
    for (section, kind), rows in sections.items():
        entity = by_id[PARKING.format(section, kind.lower())]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        assert entity["totalSpotNumber"]["value"] == int(rows[0]["PLACES"])
        assert entity["zoneCode"]["value"] == kind
        assert entity["tariffCode"]["value"] == f'{rows[0]["ID_TARIFA"]}/{rows[0]["ID_HORARIO"]}'
        assert entity["name"]["value"] == rows[0]["ADREÇA"]
        geometry = entity["location"]["value"]
        lines = [geometry["coordinates"]] if geometry["type"] == "LineString" else geometry["coordinates"]
        # The pieces of a section, in no particular order.
        assert sorted(lines) == sorted([[float(r["LONGITUD_I"]), float(r["LATITUD_I"])], [float(r["LONGITUD_F"]), float(r["LATITUD_F"])]] for r in rows)
    assert {e["zoneCode"]["value"] for e in written} == {"AZL", "VM", "VR", "DUM", "BUS"}
    assert any(e["location"]["value"]["type"] == "MultiLineString" for e in written)
    blue = next(e for e in written if e["zoneCode"]["value"] == "AZL")
    assert blue["category"]["value"] == ["blueZone", "feeCharged"] and blue["allowedVehicleType"]["value"] == ["car"]


@requires_docker
def test_an_out_of_area_piece_places_out_of_range_and_a_duplicate_piece_are_left_out_and_counted():
    text = (FIXTURES / "TRAMS.CSV").read_bytes().decode("utf-8")
    lines = text.rstrip("\n").split("\n")
    first = lines[1].split(",")
    swapped = ",".join([first[0], *first[1:5], first[6], first[5], *first[7:]])  # lat/lon swapped
    negative = lines[2].rsplit(",", 1)[0] + ",-3"
    document = "\n".join([*lines, swapped, negative, lines[3], "999,x,,,,,,,,,,,,ZZZ,,,1"]).encode()
    out, errors = parking(document, counted=True)
    assert errors == []
    assert out[0]["dropped"] == {"malformedRow": 1, "placesOutOfRange": 1, "pieceOutOfArea": 1, "duplicatePiece": 1}
    assert len(out[0]["entities"]) == 15


@pytest.mark.parametrize("document, reason", [
    (b"", "does not have the columns"),
    (b"<html><body>503</body></html>", "does not have the columns"),
    ("﻿ID_TRAM,UTM_IX,UTM_FX,UTM_IY,UTM_FY,LATITUD_I,LONGITUD_I,LATITUD_F,LONGITUD_F,ID_TARIFA,ID_HORARIO,"
     "ID_TARIFA1,ID_HORARIO1,TIPUS_TRAM,ADREÇA,COLOR_RGB,PLACES\n".encode(), "does not have the columns"),
    ("﻿ID_TRAM,LATITUD_I,LONGITUD_I,LATITUD_F,LONGITUD_F,ID_TARIFA,ID_HORARIO,TIPUS_TRAM,ADREÇA,PLACES\n"
     "1,0,0,0,0,1,1,AZL,X,1\n".encode(), "names no section"),
], ids=["empty", "html", "header-only", "only-0-0"])
@requires_docker
def test_a_file_that_is_not_the_parking_csv_or_names_no_section_fails_the_run(document, reason):
    written, errors = parking(document)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


def test_the_seed_reads_the_parking_sections_daily_and_publishes_them():
    source = yaml.safe_load((SEED / "barcelona-datasource-parking-sections.yaml").read_text())["spec"]["http"]
    assert source["url"] == f"{HOST}/resources/bsm/TRAMS.CSV"
    pipeline = yaml.safe_load((SEED / "barcelona-pipeline-parking-sections.yaml").read_text())["spec"]
    assert "period" not in pipeline and pipeline["schedule"] == "20 4 * * *" and pipeline["output"] == {"type": "OnStreetParking", "mode": "upsert"}
    # 10,596 sections of the 3.1 MB file need about 256 MB in Bento (docker --memory, 2026-10-22).
    assert pipeline["quotas"]["maxMemoryMb"] >= 256
    endpoint = yaml.safe_load((SEED / "barcelona-endpoint-parking.yaml").read_text())["spec"]
    assert endpoint["policyRef"].endswith(":public-parking") and "ckan" in endpoint["publish"]
    policy = yaml.safe_load((SEED / "barcelona-policy-public-parking.yaml").read_text())["spec"]
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["OnStreetParking"]
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    for name in ("datasource-parking-sections", "pipeline-parking-sections", "pipeline-parking-sections-bento",
                 "endpoint-parking", "policy-public-parking"):
        assert f"barcelona-{name}.yaml" in index


ROUTE = "urn:ngsi-ld:TrafficFlowObserved:barcelona.cat:barcelona:itinerari-{}"
# The route file was written at 07:55:53 Barcelona time, 05:55:53 UTC.
ROUTES_NOW = "2026-10-23T06:00:00Z"


class RouteLists(http.server.BaseHTTPRequestHandler):
    """The route list and the section list as recorded, by path, or `status` instead of both."""

    status: ClassVar[int] = 200
    routes: ClassVar[bytes] = b""
    sections: ClassVar[bytes] = b""

    def do_GET(self):
        self.send_response(RouteLists.status)
        self.send_header("Content-Type", "text/csv")
        self.end_headers()
        self.wfile.write(RouteLists.routes if "per_itinerari" in self.path else RouteLists.sections)

    def log_message(self, *_):
        pass


def routes(document: bytes, route_list: bytes | None = None, sections: bytes | None = None, status: int = 200,
           now: str = ROUTES_NOW, counted: bool = False) -> tuple[list[dict], list[str]]:
    """The traffic-routes mapping over `document` with its clock at `now`."""
    RouteLists.status = status
    RouteLists.routes = (FIXTURES / "transit_relacio_tram_per_itinerari.csv").read_bytes() if route_list is None else route_list
    RouteLists.sections = (FIXTURES / "transit_relacio_trams.csv").read_bytes() if sections is None else sections
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), RouteLists)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (SEED / "barcelona-pipeline-traffic-routes-bento.yaml").read_text()
        text = text.replace(f"{HOST}/data/dataset/", f"http://127.0.0.1:{server.server_port}/data/dataset/")
        text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
        processors = yaml.safe_load(text)["pipeline"]["processors"]
        if counted:
            steps = processors[0]["try"]
            processors = [{"try": steps[:steps.index(next(s for s in steps if "log" in s))]}]
        return bento(processors, document)
    finally:
        server.shutdown()


def route_rows() -> list[list[str]]:
    text = (FIXTURES / "itineraris.dat").read_bytes().decode()
    return [line.strip().split("#") for line in text.splitlines() if line.strip()]


def route_sections() -> dict[str, list[str]]:
    text = (FIXTURES / "transit_relacio_tram_per_itinerari.csv").read_bytes().decode("utf-8-sig")
    members: dict[str, list[str]] = {}
    for row in __import__("csv").DictReader(text.splitlines()):
        if row["Tram"] != "0":
            members.setdefault(row["Itinerari"], []).append(row["Tram"])
    return members


@requires_docker
def test_every_recorded_route_is_a_valid_traffic_flow_observed_with_its_sections_lines():
    written, errors = routes((FIXTURES / "itineraris.dat").read_bytes())
    assert errors == []
    by_id = {e["id"]: e for e in written}
    assert len(written) == len(route_rows()) == 10
    members = route_sections()
    for route, available, _at, now, usual, *_ in route_rows():
        entity = by_id[ROUTE.format(route)]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        assert entity["dateObserved"]["value"] == "2026-10-23T05:55:53Z"
        lines = entity["location"]["value"]
        assert lines["type"] == "MultiLineString" and len(lines["coordinates"]) == len(members[route])
        assert all(2.04 <= lon <= 2.24 and 41.30 <= lat <= 41.48 for line in lines["coordinates"] for lon, lat in line)
        if available == "1" and int(now) > 0:
            assert entity["travelTime"] == {"type": "Property", "value": int(now), "unitCode": "SEC", "observedAt": "2026-10-23T05:55:53Z"}
            assert entity["usualTravelTime"]["value"] == int(usual)
        else:
            # No reading: the last travel time stays as it was, with the time it was read.
            assert "travelTime" not in entity and "usualTravelTime" not in entity
    assert by_id[ROUTE.format("3")]["name"]["value"] == "Guipúscoa de Meridiana a Cantàbria"
    assert by_id[ROUTE.format("3")]["travelTime"]["value"] == 306


@requires_docker
def test_an_out_of_area_section_a_travel_time_out_of_range_and_a_duplicate_route_are_left_out_and_counted():
    rows = (FIXTURES / "itineraris.dat").read_bytes().decode().splitlines()
    # Route 15's three sections with lat/lon swapped leave the route without a line.
    sections = (FIXTURES / "transit_relacio_trams.csv").read_bytes().decode("utf-8")
    for section in route_sections()["15"]:
        line = next(s for s in sections.splitlines() if s.startswith(f"{section},"))
        coordinates = line.rsplit(',"', 1)[1].rstrip('"')
        numbers = coordinates.split(",")
        swapped = ",".join(v for pair in zip(numbers[1::2], numbers[0::2]) for v in pair)
        sections = sections.replace(line, line.replace(coordinates, swapped))
    extra = ["4#1#20261023075553#9999#300#1#1#1", "5#1#20261023075553#100#100#1#1#1", "x#y",
             "6#1#20261026075553#100#100#1#1#1"]
    out, errors = routes("\r\n".join(rows + extra).encode(), sections=sections.encode(), counted=True)
    assert errors == []
    assert out[0]["dropped"] == {
        "malformedLine": 1, "travelTimeOutOfRange": 1, "timeInFuture": 1, "duplicateRoute": 1,
        "routeWithoutLine": 1, "lineMalformed": 0, "lineOutOfArea": 3,
    }
    ids = [e["id"] for e in out[0]["entities"]]
    assert ROUTE.format("15") not in ids and len(ids) == len(set(ids)) == 9
    # The duplicate 5 is the second line: the first, as recorded, is kept.
    five = next(e for e in out[0]["entities"] if e["id"] == ROUTE.format("5"))
    assert five["travelTime"]["value"] == 174


@pytest.mark.parametrize("document, route_list, status, now, reason", [
    (b"", None, 200, ROUTES_NOW, "names no route"),
    (b"<html>302 Found</html>", None, 200, ROUTES_NOW, "names no route"),
    (None, None, 200, "2026-10-23T06:20:00Z", "older than three 5-minute periods"),
    (None, None, 503, ROUTES_NOW, ""),
    (None, b"id;name\r\n1;x\r\n", 200, ROUTES_NOW, "Itinerari, Tram, Descripci"),
    (b" " * 262145, None, 200, ROUTES_NOW, "more than the 256 kB read"),
], ids=["empty", "html", "stale", "lists-503", "route-columns", "oversize"])
@requires_docker
def test_an_empty_stale_or_broken_route_source_fails_the_run_and_writes_nothing(document, route_list, status, now, reason):
    document = (FIXTURES / "itineraris.dat").read_bytes() if document is None else document
    written, errors = routes(document, route_list, status=status, now=now)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


def test_the_seed_reads_the_routes_every_five_minutes_from_the_city_hosts_only():
    source = yaml.safe_load((SEED / "barcelona-datasource-traffic-routes.yaml").read_text())["spec"]["http"]
    # The dataset's download link answers 302 to this file.
    assert source["url"] == "https://www.bcn.cat/transit/dades/dadesitineraris.dat"
    pipeline = yaml.safe_load((SEED / "barcelona-pipeline-traffic-routes.yaml").read_text())["spec"]
    assert pipeline["period"] == "300s"
    assert pipeline["output"] == {"type": "TrafficFlowObserved", "mode": "upsert"}
    text = (SEED / "barcelona-pipeline-traffic-routes-bento.yaml").read_text()
    urls = [line.split("url:", 1)[1].strip() for line in text.splitlines() if line.strip().startswith("url:")]
    assert len(urls) == 2 and all(u.startswith(f"{HOST}/data/dataset/") for u in urls)
    endpoint = yaml.safe_load((SEED / "barcelona-endpoint-traffic.yaml").read_text())["spec"]
    assert f"{HOST}/data/ca/dataset/itineraris" in [s["url"] for s in endpoint["catalog"]["source"]]
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    for name in ("datasource-traffic-routes", "pipeline-traffic-routes", "pipeline-traffic-routes-bento"):
        assert f"barcelona-{name}.yaml" in index


CHARGING = "urn:ngsi-ld:EVChargingStation:barcelona.cat:barcelona:endolla-{}"
# The newest port change of the recorded file is 2026-10-23T07:55:55+02:00 (05:55:55 UTC).
CHARGING_NOW = "2026-10-23T06:05:00Z"


def charging(document: bytes, now: str = CHARGING_NOW, counted: bool = False) -> tuple[list[dict], list[str]]:
    text = (SEED / "barcelona-pipeline-ev-charging-bento.yaml").read_text()
    text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
    processors = yaml.safe_load(text)["pipeline"]["processors"]
    if counted:
        steps = processors[0]["try"]
        processors = [{"try": steps[:steps.index(next(s for s in steps if "log" in s))]}]
    return bento(processors, document)


def points() -> list[dict]:
    return json.loads((FIXTURES / "informacio-punts-recarrega-electric.json").read_bytes())["locations"]


@requires_docker
def test_every_recorded_charging_point_is_a_valid_ev_charging_station_with_its_free_ports():
    written, errors = charging((FIXTURES / "informacio-punts-recarrega-electric.json").read_bytes())
    assert errors == []
    by_id = {e["id"]: e for e in written}
    assert len(written) == len(points()) == 15
    for point in points():
        entity = by_id[CHARGING.format(point["id"])]
        assert not open_data.schema_errors(SCHEMA, entity), entity
        ports = [p for s in point["stations"] for p in s["ports"]]
        states = [p["port_status"][0]["status"] for p in ports]
        assert entity["capacity"]["value"] == entity["socketNumber"]["value"] == len(ports)
        assert entity["availableCapacity"]["value"] == states.count("AVAILABLE") <= len(ports)
        assert entity["status"]["value"] == ("working" if "AVAILABLE" in states else "full" if "IN_USE" in states else "outOfService")
        assert entity["location"]["value"] == {"type": "Point", "coordinates": [point["coordinates"]["longitude"], point["coordinates"]["latitude"]]}
        assert entity["name"]["value"] == point["address"]["address_string"].strip()
        assert entity["openingHours"]["value"] == "Mo-Su 00:00-24:00"
    assert {e["status"]["value"] for e in written} == {"working", "outOfService"}
    fast = by_id[CHARGING.format("3817")]
    assert fast["socketType"]["value"] == ["CCS/SAE", "CHAdeMO", "Type2"] and fast["allowedVehicleType"]["value"] == ["car"]
    # Household sockets: a motorcycle bay on the street, cars and motorcycles in a car park.
    assert by_id[CHARGING.format("3763")]["allowedVehicleType"]["value"] == ["motorcycle"]
    assert by_id[CHARGING.format("3757")]["allowedVehicleType"]["value"] == ["car", "motorcycle"]
    assert by_id[CHARGING.format("3757")]["socketType"]["value"] == ["Wall_Euro"]
    assert by_id[CHARGING.format("3758")]["socketType"]["value"] == ["Type2"]


@requires_docker
def test_an_out_of_area_point_a_port_out_of_range_and_a_duplicate_point_are_left_out_and_counted():
    recorded_points = points()
    swapped = json.loads(json.dumps(recorded_points[0]))
    swapped["id"] = "9001"
    swapped["coordinates"] = {"latitude": swapped["coordinates"]["longitude"], "longitude": swapped["coordinates"]["latitude"]}
    again = json.loads(json.dumps(recorded_points[1]))
    again["id"] = "9002"  # the same address and place under another id
    over = json.loads(json.dumps(recorded_points[2]))
    over["stations"][0]["ports"][0]["power_kw"] = 4000
    full = json.loads(json.dumps(recorded_points[3]))
    for station in full["stations"]:
        for port in station["ports"]:
            port["port_status"] = [{"status": "IN_USE"}]
    document = {"locations": [*recorded_points[:2], over, full, *recorded_points[4:], swapped, again, recorded_points[5], {"id": "x"}]}
    out, errors = charging(json.dumps(document).encode(), counted=True)
    assert errors == []
    assert out[0]["dropped"] == {"pointMalformed": 1, "pointOutOfArea": 1, "portOutOfRange": 1, "pointWithoutPort": 0,
                                 "timeInFuture": 0, "duplicatePoint": 2}
    by_id = {e["id"]: e for e in out[0]["entities"]}
    assert len(by_id) == 15 and CHARGING.format("9001") not in by_id and CHARGING.format("9002") not in by_id
    assert by_id[CHARGING.format(over["id"])]["capacity"]["value"] == 2
    assert by_id[CHARGING.format(full["id"])]["status"]["value"] == "full"
    assert by_id[CHARGING.format(full["id"])]["availableCapacity"]["value"] == 0


@pytest.mark.parametrize("document, now, reason", [
    (b"", CHARGING_NOW, "names no charging point"),
    (b"<html><body>503</body></html>", CHARGING_NOW, "names no charging point"),
    (b'{"gelfs_version":"0.96","locations":[]}', CHARGING_NOW, "names no charging point"),
    (b'{"locations":[{"id":"1","coordinates":{"latitude":0,"longitude":0},"stations":[]}]}', CHARGING_NOW, "no usable charging point"),
    (None, "2026-10-23T12:00:00Z", "more than six hours ago"),
    (b" " * 2097153, CHARGING_NOW, "more than the 2 MB read"),
], ids=["empty", "html", "no-point", "only-0-0", "stale", "oversize"])
@requires_docker
def test_a_charging_file_that_is_empty_stale_or_broken_fails_the_run_and_writes_nothing(document, now, reason):
    document = (FIXTURES / "informacio-punts-recarrega-electric.json").read_bytes() if document is None else document
    written, errors = charging(document, now)
    assert written == []
    assert len(errors) == 1 and reason in errors[0], errors


def test_the_seed_reads_the_charging_points_every_five_minutes_and_publishes_them():
    source = yaml.safe_load((SEED / "barcelona-datasource-ev-charging.yaml").read_text())["spec"]["http"]
    assert source["url"].startswith(f"{HOST}/data/dataset/")
    pipeline = yaml.safe_load((SEED / "barcelona-pipeline-ev-charging.yaml").read_text())["spec"]
    assert pipeline["period"] == "300s" and pipeline["output"] == {"type": "EVChargingStation", "mode": "upsert"}
    assert "url:" not in (SEED / "barcelona-pipeline-ev-charging-bento.yaml").read_text()
    endpoint = yaml.safe_load((SEED / "barcelona-endpoint-charging.yaml").read_text())["spec"]
    assert endpoint["policyRef"].endswith(":public-charging") and "ckan" in endpoint["publish"]
    policy = yaml.safe_load((SEED / "barcelona-policy-public-charging.yaml").read_text())["spec"]
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["EVChargingStation"]
    assert not {"upsertBatch", "createBatch", "deleteBatch"} & set(policy["operations"])
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    for name in ("datasource-ev-charging", "pipeline-ev-charging", "pipeline-ev-charging-bento",
                 "endpoint-charging", "policy-public-charging"):
        assert f"barcelona-{name}.yaml" in index
