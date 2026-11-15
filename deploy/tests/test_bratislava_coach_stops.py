"""The city's list of tourist coach stops becomes Smart Data Models OnStreetParking, each stop placed
by its name, checked and counted (T-5435).

Bento runs the committed mapping over the list recorded from data.bratislava.sk on 2026-10-25
(`fixtures/bratislava/hub/coach-stops.json`, 13 stops). The tests add the rows the owner's validation
asks for (2026-10-22T18:10Z): a stop the placing table does not know (a name outside the city), a
count of places out of range and a stop twice, each dropped with a counted reason. Without a
container runtime the Bento cases skip.
"""

import copy
import re

import yaml
from open_data import requires_docker
from test_bratislava_hub import SEED, fixture, logged, run, valid

import pytest

pytestmark = pytest.mark.xdist_group("docker-bratislava-hub")

MAPPING = "bratislava-pipeline-coach-stops-bento.yaml"
PATH = "List_of_Tourist_Buses_in_2025_"
BBOX = (16.93, 47.99, 17.3, 48.28)


def stops(extra: list[dict] | None = None) -> tuple[dict, str]:
    answer = copy.deepcopy(fixture("coach-stops.json"))
    answer["features"] += [{"attributes": a} for a in extra or []]
    entities, log = run(MAPPING, fixture("services.json"), {PATH: answer})
    return {e["id"]: e for e in entities}, log


def urn(local: str) -> str:
    return f"urn:ngsi-ld:OnStreetParking:bratislava.sk:bratislava:{local}"


def test_every_placed_stop_lies_in_bratislava():
    text = (SEED / MAPPING).read_text()
    points = re.findall(r'\[(4[0-9.]+), (1[0-9.]+), "', text)
    assert len(points) == 13
    for lat, lon in points:
        assert BBOX[0] <= float(lon) <= BBOX[2] and BBOX[1] <= float(lat) <= BBOX[3], (lat, lon)


@requires_docker
def test_the_recorded_list_maps_to_13_valid_coach_stops():
    by_id, log = stops()
    assert len(by_id) == 13
    assert logged(log) == {}
    for stop in by_id.values():
        valid(stop, "dataModel.Parking/OnStreetParking")
        assert stop["allowedVehicleType"]["value"] == ["bus"]
    castle = by_id[urn("coach-stop-hrad-castle")]
    assert castle["category"]["value"] == ["shortTerm"]
    assert castle["location"]["value"]["coordinates"] == [17.094782, 48.144314]
    assert "Mudroňova" in castle["description"]["value"]
    cemetery = by_id[urn("coach-stop-slavicie-udolie-cemetery")]
    assert cemetery["category"]["value"] == ["mediumTerm"]
    assert cemetery["totalSpotNumber"]["value"] == 9


@requires_docker
def test_an_unplaced_stop_a_bad_count_and_a_repeat_are_dropped_by_reason():
    rows = fixture("coach-stops.json")["features"]
    lovinskeho = next(r["attributes"] for r in rows if r["attributes"]["Name"] == "Lovinského")
    by_id, log = stops([
        dict(lovinskeho, Name="Wien Praterstern"),
        dict(lovinskeho, Name="Pasienky", Count_=-4),
        dict(lovinskeho),
    ])
    assert logged(log) == {"not placed": 1, "out of range": 1, "duplicate": 1}
    assert "no place for Wien Praterstern" in log
    assert len(by_id) == 13
    assert by_id[urn("coach-stop-lovinskeho")]["totalSpotNumber"]["value"] == 2


@requires_docker
def test_a_list_the_hub_no_longer_names_writes_nothing():
    entities, log = run(MAPPING, {"services": [{"name": "Iný_zoznam"}]}, {})
    assert entities == [] and "no longer lists the coach stops" in log


def test_the_pipeline_writes_onstreetparking_through_the_door():
    manifest = yaml.safe_load((SEED / "bratislava-pipeline-coach-stops.yaml").read_text())
    assert manifest["spec"]["output"] == {"type": "OnStreetParking", "mode": "upsert"}
    assert manifest["spec"]["source"]["dataSourceRef"]["name"] == "bratislava-hub"
