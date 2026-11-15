"""Dopravný podnik Bratislava's GTFS timetable becomes GtfsAgency, GtfsRoute and GtfsStop (T-5273, PL-03, PL-72).

Bento runs the committed mhd-gtfs mapping over a cut of each member it reads from the hub's zip,
fetched on 2026-10-24 (`fixtures/bratislava/gtfs`: agency.txt whole, the first 20 lines and 40
platforms), with the member name the archive read sets. Edited for the checks: the fourth line's
route type set to 7 (a funicular DPB does not run) and the second line repeated under a new
route_id; the sixth platform moved to Vienna, the seventh with its latitude and longitude swapped,
and the first repeated under a new stop_id.

Without a container runtime the Bento cases skip.
"""

import csv
import io
from pathlib import Path

import open_data
import pytest
import test_bratislava_counters
import yaml

pytestmark = pytest.mark.xdist_group("docker-bratislava-gtfs")

BRATISLAVA = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/bratislava"
FIXTURES = Path(__file__).resolve().parent / "fixtures/bratislava/gtfs"
SCHEMA = BRATISLAVA / "bratislava.v1.schema.json"
MAPPING = "bratislava-pipeline-mhd-gtfs-bento.yaml"


def run(member: str, text: str | None = None) -> tuple[dict, list[str]]:
    test_bratislava_counters.needs_docker()
    document = (FIXTURES / member).read_text() if text is None else text
    written, logs, _ = test_bratislava_counters.run(MAPPING, document, [], {"archive_filename": member})
    kind = {"agency.txt": "GtfsAgency", "routes.txt": "GtfsRoute", "stops.txt": "GtfsStop"}[member]
    return test_bratislava_counters.by_id(written, kind), logs


def rows(member: str) -> list[dict]:
    return list(csv.DictReader(io.StringIO((FIXTURES / member).read_text())))


def checked(entities: dict, model: str):
    for e in entities.values():
        assert open_data.sdm_errors(f"dataModel.UrbanMobility/{model}", e) == [], e["id"]
        assert open_data.schema_errors(SCHEMA, e) == [], e["id"]


def test_the_agency_is_dpb_and_valid():
    agencies, logs = run("agency.txt")
    assert list(agencies) == ["dpb-01"] and logs == []
    dpb = agencies["dpb-01"]
    assert dpb["agencyName"]["value"] == "Dopravný podnik Bratislava, a.s."
    assert dpb["timezone"]["value"] == "Europe/Bratislava" and dpb["page"]["value"] == "http://www.dpb.sk"
    checked(agencies, "GtfsAgency")


def test_one_line_per_number_and_the_edited_rows_are_left_out_with_their_reason():
    lines, logs = run("routes.txt")
    source = rows("routes.txt")
    assert len(lines) == len(source) - 2
    assert f"dpb-line-{source[3]['route_short_name'].lower()}" not in lines, "a route type DPB does not run was written"
    one = lines["dpb-line-1"]
    assert one["shortName"]["value"] == "1" and one["routeType"]["value"] == "0" and one["transportMode"]["value"] == "tram"
    assert one["routeColor"]["value"] == "F56200"
    assert one["operatedBy"] == {"type": "Relationship", "object": "urn:ngsi-ld:GtfsAgency:bratislava.sk:bratislava:dpb-01"}
    assert "name" not in one, "route_long_name is DPB's variant note, not a name"
    trolleybuses = [e for e in lines.values() if e["transportMode"]["value"] == "trolleybus"]
    assert trolleybuses and all(e["routeType"]["value"] == "3" for e in trolleybuses)
    assert logs == ['warning: MHD GTFS rows left out: routes.txt {"duplicate":1,"unknownRouteType":1}']
    checked(lines, "GtfsRoute")


def test_one_stop_per_platform_in_bratislava_and_the_edited_rows_are_left_out_with_their_reason():
    stops, logs = run("stops.txt")
    source = rows("stops.txt")
    assert len(stops) == len(source) - 3
    assert f"dpb-{source[5]['stop_id']}" not in stops and f"dpb-{source[6]['stop_id']}" not in stops
    assert f"dpb-{source[-1]['stop_id']}" not in stops, "the repeat of the first platform was written"
    first = stops[f"dpb-{source[0]['stop_id']}"]
    assert first["name"] == {"type": "Property", "value": "Hlavná stanica"}
    assert first["stopCode"]["value"] == "93" and first["platformCode"]["value"] == "A" and first["zoneCode"]["value"] == "100"
    assert first["location"]["value"]["coordinates"] == [float(source[0]["stop_lon"]), float(source[0]["stop_lat"])]
    assert logs == ['warning: MHD GTFS rows left out: stops.txt {"duplicate":1,"outsideBratislava":1,"swappedCoordinates":1}']
    checked(stops, "GtfsStop")


@pytest.mark.parametrize("member", ["agency.txt", "routes.txt", "stops.txt"])
def test_a_member_with_only_its_header_fails_and_writes_nothing(member):
    header = (FIXTURES / member).read_text().splitlines()[0] + "\n"
    written, logs = run(member, header)
    assert written == {} and any(log.startswith("error") and "kept no row" in log for log in logs), logs


def test_a_stops_file_whose_platforms_mostly_fall_outside_fails_and_writes_nothing():
    text = (FIXTURES / "stops.txt").read_text().replace(",48.1", ",8.1")
    written, logs = run("stops.txt", text)
    assert written == {} and any(log.startswith("error") and "kept only" in log for log in logs), logs


def test_the_source_reads_three_members_of_the_zip_within_a_bound_and_writes_through_the_door():
    source = yaml.safe_load((BRATISLAVA / "bratislava-datasource-mhd-gtfs.yaml").read_text())["spec"]["http"]
    assert source["archive"]["members"] == ["agency.txt", "routes.txt", "stops.txt"]
    assert 0 < source["archive"]["maxBytes"] <= 64 * 1024 * 1024 and source["timeout"] == "60s"
    pipeline = yaml.safe_load((BRATISLAVA / "bratislava-pipeline-mhd-gtfs.yaml").read_text())["spec"]
    assert pipeline["source"]["dataSourceRef"]["name"] == "dpb-gtfs"
    assert pipeline["output"]["mode"] == "upsert"
    # One run over the real zip is killed at 64 MiB and passes at 96 (T-5766).
    assert pipeline["quotas"]["maxMemoryMb"] >= 96
