"""Fintraffic LIIPI's park and ride sites of the HSL area become OffStreetParking (T-3894).

Bento runs the committed mappings over answers recorded on 2026-10-12 (`fixtures/helsinki/liipi`:
46 of the 545 facilities of the HSL area, every status and kind among them; the country's 39
utilisation rows; the 122 stations of HSL's GTFS). The calls to the gateway are the cut: the
test answers the ask for stations and for the sites the space holds.
"""

import copy
import json
from datetime import datetime, timedelta, timezone

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_helsinki_departures import HELSINKI, docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

FIXTURES = HELSINKI.parents[3] / "tests/fixtures/helsinki/liipi"
URN = "urn:ngsi-ld:OffStreetParking:hel.fi:helsinki:liipi-{}"
STATION = "urn:ngsi-ld:GtfsStation:hel.fi:helsinki:{}"


def facilities() -> dict:
    return json.loads((FIXTURES / "facilities.json").read_text())


def stations() -> list[dict]:
    return [{"id": s["id"], "lon": s["location"]["coordinates"][0], "lat": s["location"]["coordinates"][1]}
            for s in json.loads((FIXTURES / "stations.json").read_text())]


def run(mapping: str, document: bytes, answer: str) -> list[dict]:
    """The mapping with its gateway branch answered by `answer`, a mutation."""
    steps = [{"mutation": answer} if "branch" in p else p
             for p in yaml.safe_load((HELSINKI / mapping).read_text())["pipeline"]["processors"]]
    out = docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps}}, document)
    written = [json.loads(line) for line in out.decode().splitlines() if line.strip()]
    for entity in written:
        assert entity["type"] == "OffStreetParking"
    return written


def sites(document: dict, near: list[dict] | None = None) -> dict[str, dict]:
    written = run("helsinki-pipeline-liipi-facilities-bento.yaml", json.dumps(document).encode(),
                  f"root.stations = {json.dumps(stations() if near is None else near)}")
    for entity in written:
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
    return {e["id"].rsplit("-", 1)[1]: e for e in written}


def free(document: bytes, held: list[str] | None) -> dict[str, dict]:
    answer = 'root.known = {"failed": true}' if held is None else f"root.known = {json.dumps({'held': held})}"
    return {e["id"].rsplit("-", 1)[1]: e for e in run("helsinki-pipeline-liipi-utilizations-bento.yaml", document, answer)}


@requires_docker
def test_every_recorded_facility_is_a_valid_site_with_status_capacity_and_station():
    written = sites(facilities())
    assert len(written) == 46
    by_status = {}
    for f in facilities()["results"]:
        by_status.setdefault(f["status"], set()).add(written[str(f["id"])]["status"]["value"])
    assert by_status == {"IN_OPERATION": {"open"}, "TEMPORARILY_CLOSED": {"closed"}, "INACTIVE": {"closed"},
                         "EXCEPTIONAL_SITUATION": {"closedAbnormal"}}
    car = next(e for e in written.values() if "disabled" in e["capacityByType"]["json"])
    assert car["allowedVehicleType"]["value"][0] == "car"
    assert car["totalSpotNumber"]["value"] == car["capacityByType"]["json"]["car"]
    bike = next(e for e in written.values() if e["allowedVehicleType"]["value"] == ["bicycle"])
    assert bike["totalSpotNumber"]["value"] == bike["capacityByType"]["json"]["bicycle"]
    # The register carries no free spaces; those come from the utilisations alone.
    assert not any("availableSpotNumber" in e for e in written.values())
    assert all(e["refNearestStation"]["object"].startswith(STATION.format(""))
               for e in written.values() if "refNearestStation" in e)


@requires_docker
def test_the_nearest_station_within_500_m_is_linked_and_one_further_is_not():
    one = copy.deepcopy(facilities()["results"][0])
    lon, lat = one["location"]["bbox"][0], one["location"]["bbox"][1]
    one["location"] = {"type": "Point", "coordinates": [lon, lat], "bbox": [lon, lat, lon, lat]}
    # 0.0045° of latitude is about 500 m: 300 m north, 450 m south, 600 m east.
    near = [{"id": STATION.format("north"), "lon": lon, "lat": lat + 0.0027},
            {"id": STATION.format("south"), "lon": lon, "lat": lat - 0.00405}]
    far = [{"id": STATION.format("east"), "lon": lon + 0.0108, "lat": lat}]
    assert sites({"results": [one]}, near + far)[str(one["id"])]["refNearestStation"]["object"] == STATION.format("north")
    assert "refNearestStation" not in sites({"results": [one]}, far)[str(one["id"])]
    # The ask for stations failed: the site is still written, only without its link.
    assert "refNearestStation" not in sites({"results": [one]}, [])[str(one["id"])]


@requires_docker
def test_a_bad_answer_writes_no_site():
    assert sites({"results": []}) == {}
    assert run("helsinki-pipeline-liipi-facilities-bento.yaml", b"<html>503</html>", "root.stations = []") == []
    broken = copy.deepcopy(facilities()["results"][0])
    del broken["location"]
    assert sites({"results": [broken]}) == {}


@requires_docker
def test_free_spaces_go_only_to_held_sites_and_a_stale_counter_is_left_out():
    rows = json.loads((FIXTURES / "utilizations.json").read_text())
    document = json.dumps(rows).encode()
    held = [URN.format(i) for i in (1047, 1205, 278)]
    written = free(document, held)
    # 1047 last reported in May 2024: not live, so not written.
    assert set(written) == {"1205", "278"}
    for facility in ("1205", "278"):
        row = next(r for r in rows if str(r["facilityId"]) == facility)
        spaces = written[facility]["availableSpotNumber"]
        assert spaces["value"] == row["spacesAvailable"]
        assert spaces["observedAt"].startswith(row["timestamp"][:10])
        assert set(written[facility]) == {"id", "type", "availableSpotNumber"}
    # A site with two kinds of space sums them, observed at the newest row.
    two = [{"facilityId": 9, "capacityType": kind, "usage": "PARK_AND_RIDE", "timestamp": f"2026-10-12T06:4{n}:00.000+03:00",
            "spacesAvailable": n + 3, "capacity": 20, "openNow": True} for n, kind in enumerate(("CAR", "DISABLED"))]
    summed = free(json.dumps(two).encode(), [URN.format(9)])["9"]["availableSpotNumber"]
    assert summed == {"type": "Property", "value": 7, "observedAt": "2026-10-12T03:41:00Z"}


@requires_docker
def test_a_failed_ask_or_a_bad_body_writes_no_free_spaces():
    document = (FIXTURES / "utilizations.json").read_bytes()
    assert free(document, None) == {}
    assert free(document, []) == {}
    assert free(b'{"error": "maintenance"}', [URN.format(1205)]) == {}
    assert free(b"\x20" * (1024 * 1024 + 1), [URN.format(1205)]) == {}


@requires_docker
def test_the_ask_names_exactly_the_reporting_sites():
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-liipi-utilizations-bento.yaml").read_text())["pipeline"]["processors"]
    branch = next(p for p in steps if "branch" in p)
    ask = [*steps[:steps.index(branch)], {"mapping": branch["branch"]["request_map"]}]
    out = docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": ask}},
                 (FIXTURES / "utilizations.json").read_bytes())
    asked = json.loads(out)
    rows = json.loads((FIXTURES / "utilizations.json").read_text())
    seen: dict[int, datetime] = {}
    for r in rows:
        seen[r["facilityId"]] = max(seen.get(r["facilityId"], datetime.min.replace(tzinfo=timezone.utc)), datetime.fromisoformat(r["timestamp"]))
    newest = max(seen.values())
    # Three counters stopped long ago (2023, 2024, a month before): a day is the bound.
    live = {facility for facility, at in seen.items() if newest - at <= timedelta(days=1)}
    assert len(seen) - len(live) == 3
    assert asked["type"] == "Query" and {e["type"] for e in asked["entities"]} == {"OffStreetParking"}
    assert {e["id"] for e in asked["entities"]} == {URN.format(i) for i in live}


def test_the_seed_reads_liipi_for_the_hsl_area_and_publishes_the_sites():
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-liipi-facilities.yaml").read_text())["spec"]["http"]
    assert source["url"].startswith("https://parking.fintraffic.fi/api/v1/facilities?geometry=POLYGON")
    utilisation = yaml.safe_load((HELSINKI / "helsinki-datasource-liipi-utilizations.yaml").read_text())["spec"]["http"]
    assert utilisation["url"] == "https://parking.fintraffic.fi/api/v1/utilizations"
    daily = yaml.safe_load((HELSINKI / "helsinki-pipeline-liipi-facilities.yaml").read_text())["spec"]
    live = yaml.safe_load((HELSINKI / "helsinki-pipeline-liipi-utilizations.yaml").read_text())["spec"]
    # After the GTFS stations (04:20), so the first run has stations to link.
    assert daily["schedule"] == "40 4 * * *" and live["period"] == "300s"
    assert daily["output"] == live["output"] == {"type": "OffStreetParking", "mode": "upsert"}
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert {f"helsinki-{kind}-liipi-{name}.yaml" for kind in ("datasource", "pipeline")
            for name in ("facilities", "utilizations")} <= set(index)
    public = yaml.safe_load((HELSINKI / "helsinki-policy-public-all.yaml").read_text())
    assert "OffStreetParking" in {e["type"] for rule in public["spec"]["information"] for e in rule["entities"]}
