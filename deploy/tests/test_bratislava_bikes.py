"""Slovnaft BAjk, nextbike's Bratislava system, becomes BikeHireDockingStation (T-5273, PL-03, DM-01).

Bento runs the committed bike-stations and bike-availability mappings over the first 30 stations of
nextbike's GBFS 2.3 feed `nextbike_bi` recorded on 2026-10-23 at 06:46 UTC
(`fixtures/bratislava/nextbike`, the stations' `station_area` polygons cut). Four records were
edited for the checks: the second station moved to Vienna, the third with its latitude and longitude
swapped, the first repeated 4 m away under a new id, and the fourth's status reporting -3 bikes.
The feed's `last_updated` is set to now, as the live feed's is; one case makes it an hour old.

Without a container runtime the Bento cases skip.
"""

import json
import shutil
import time
from pathlib import Path

import open_data
import pytest
import test_bratislava_counters

pytestmark = pytest.mark.xdist_group("docker-bratislava-bikes")

BRATISLAVA = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/bratislava"
FIXTURES = Path(__file__).resolve().parent / "fixtures/bratislava/nextbike"
SCHEMA = BRATISLAVA / "bratislava.v1.schema.json"
URN = "urn:ngsi-ld:BikeHireDockingStation:bratislava.sk:bratislava:nextbike-"


def feed(name: str, age: int = 0) -> bytes:
    document = json.loads((FIXTURES / f"{name}.json").read_text())
    document["last_updated"] = int(time.time()) - age
    return json.dumps(document).encode()


def run(mapping: str, document: bytes) -> tuple[list[dict], list[str]]:
    """The entities the mapping writes and the messages it logs (the counters' harness, no branch)."""
    if shutil.which("docker") is None:
        pytest.skip("no container runtime, so Bento cannot run the mapping")
    entities, logs, _ = test_bratislava_counters.run(mapping, document.decode(), [])
    return entities, logs


def test_each_station_in_bratislava_is_one_entity_and_the_edited_ones_are_left_out():
    written, logs = run("bratislava-pipeline-bike-stations-bento.yaml", feed("station_information"))
    entities = {e["id"].removeprefix(URN): e for e in written}
    stations = json.loads((FIXTURES / "station_information.json").read_text())["data"]["stations"]
    assert len(entities) == len(stations) - 3
    assert stations[1]["station_id"] not in entities and stations[2]["station_id"] not in entities
    assert "999000001" not in entities, "the repeat 4 m away was written"
    first = entities[stations[0]["station_id"]]
    assert first["name"] == {"type": "Property", "value": "TESCO Express Martinčekova"}
    assert first["location"]["value"]["coordinates"] == [stations[0]["lon"], stations[0]["lat"]]
    assert "totalSlotNumber" not in first, "the feed names no capacity for a virtual station"
    assert logs == ['warning: bike stations left out: {"duplicate":1,"outsideBratislava":1,"swappedCoordinates":1}']
    for e in entities.values():
        assert open_data.sdm_errors("dataModel.Transportation/BikeHireDockingStation", e) == [], e["id"]
        assert open_data.schema_errors(SCHEMA, e) == [], e["id"]


def test_availability_is_written_onto_the_station_and_an_impossible_count_is_left_out():
    written, logs = run("bratislava-pipeline-bike-availability-bento.yaml", feed("station_status"))
    entities = {e["id"].removeprefix(URN): e for e in written}
    status = json.loads((FIXTURES / "station_status.json").read_text())["data"]["stations"]
    broken = next(s["station_id"] for s in status if s["num_bikes_available"] < 0)
    assert broken not in entities and len(entities) == len(status) - 1
    one = entities[status[0]["station_id"]]
    assert one["availableBikeNumber"]["value"] == status[0]["num_bikes_available"]
    assert one["status"]["value"] == "working" and set(one) >= {"freeSlotNumber", "dateModified"}
    assert logs == ['warning: bike availability left out: {"outOfRange":1}']
    for e in entities.values():
        assert open_data.sdm_errors("dataModel.Transportation/BikeHireDockingStation", e) == [], e["id"]
        assert open_data.schema_errors(SCHEMA, e) == [], e["id"]


@pytest.mark.parametrize("mapping, name", [("bratislava-pipeline-bike-stations-bento.yaml", "station_information"),
                                           ("bratislava-pipeline-bike-availability-bento.yaml", "station_status")])
def test_a_feed_an_hour_old_fails_and_writes_nothing(mapping, name):
    written, logs = run(mapping, feed(name, age=3600))
    assert written == [] and any(log.startswith("error") and " s old" in log for log in logs), logs
