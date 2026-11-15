"""The PID vehicles of Prague from Golemio's vehicle positions (T-5268, PL-03).

Bento runs the committed mapping over seven real vehicles recorded from
https://api.golemio.cz/v2/vehiclepositions on 2026-10-22 at 18:10Z (`fixtures/praha/golemio-pid-vehicles.json`):
a tram, a metro train and a train without a fleet number, a trolleybus, a regional bus outside
Prague and one ARRIVA bus Golemio listed twice. The edge cases are that answer edited in the one
place they name. Each entity is checked against the space's generated JSON Schema, which the gateway
enforces.
"""

import copy
import json
import shutil
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-vehicles")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
MAPPING = PRAHA / "praha-pipeline-pid-vehicles-bento.yaml"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/golemio-pid-vehicles.json").read_text())
SPACE = "praha-mesto"
NOW = "2026-10-22T18:11:00Z"


def outcome(document: dict, now: str = NOW) -> tuple[list[dict], list[str]]:
    """The mapping over one answer, with now() pinned: the entities written and the error of a
    message the runner refuses (an errored message fails the run, nothing is written)."""
    text = MAPPING.read_text().replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
    pipeline = yaml.safe_load(text)
    pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run(pinned, json.dumps(document).encode(), SPACE)
    if isinstance(answer, dict) and "__error" in answer:
        return [], [answer["__error"]]
    return answer, []


def run(document: dict, now: str = NOW) -> list[dict]:
    entities, errors = outcome(document, now)
    assert not errors, errors
    return entities


def edited(edit) -> dict:
    document = copy.deepcopy(RECORDED)
    edit(document["features"])
    return document


def by_key(entities: list[dict]) -> dict[str, dict]:
    return {e["id"].rsplit(":", 1)[1]: e for e in entities}


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return by_key(run(RECORDED))


@requires_docker
def test_every_vehicle_inside_prague_is_one_vehicle_the_gateway_accepts(written):
    # Seven reports: one outside Prague and one duplicate leave five vehicles.
    assert sorted(written) == [
        "arriva-city-kladno-8996",
        "dp-praha-514",
        "dp-praha-9099",
        "trip-1116-752-261020",
        "trip-992-129-260323",
    ], sorted(written)
    for entity in written.values():
        assert entity["type"] == "Vehicle"
        assert entity["id"].startswith(f"urn:ngsi-ld:Vehicle:hel.fi:{SPACE}:")
        assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), (entity["id"], open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity))


@requires_docker
def test_a_tram_carries_its_line_heading_next_stop_and_delay(written):
    tram = written["dp-praha-9099"]
    assert tram["vehicleType"]["value"] == "tram" and tram["transportMode"]["value"] == "tram"
    assert tram["category"]["value"] == ["public", "tracked"]
    assert tram["lineName"]["value"] == "17" and tram["direction"]["value"] == "Libuš"
    assert tram["fleetVehicleId"]["value"] == "9099" and tram["operatorName"]["value"] == "DP PRAHA"
    assert tram["nextStop"]["value"].startswith("U")
    assert isinstance(tram["delay"]["value"], int) and tram["delay"]["unitCode"] == "SEC"
    # Golemio's +02:00 stamp, written in UTC; the vehicle expires five minutes after it.
    assert tram["dateObserved"]["value"] == "2026-10-22T18:10:10Z"
    assert tram["location"]["observedAt"] == "2026-10-22T18:10:10Z"
    assert tram["expiresAt"] == "2026-10-22T18:15:10Z"
    assert tram["name"]["languageMap"]["cs"] == "Linka 17, směr Libuš"


@requires_docker
def test_metro_and_trains_without_a_fleet_number_are_keyed_by_their_trip(written):
    trips = [e for k, e in written.items() if k.startswith("trip-")]
    assert {e["transportMode"]["value"] for e in trips} == {"metro", "train"}
    assert all(e["vehicleType"]["value"] == "other" and "fleetVehicleId" not in e for e in trips)
    assert written["dp-praha-514"]["vehicleType"]["value"] == "trolley"


@requires_docker
def test_out_of_area_zero_and_swapped_positions_are_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["geometry"]["coordinates"] = list(reversed(features[1]["geometry"]["coordinates"]))
    keys = by_key(run(edited(edit)))
    # The recorded regional bus, the 0,0 tram and the swapped metro train all stay out.
    assert len(keys) == 3 and "dp-praha-9099" not in keys
    assert not any(k.startswith("csad-polkost") for k in keys)


@requires_docker
def test_a_duplicate_report_keeps_the_newer_position():
    def edit(features):
        twins = [f for f in features if f["properties"]["trip"]["vehicle_registration_number"] == 8996]
        twins[1]["properties"]["last_position"]["origin_timestamp"] = "2026-10-22T20:10:30+02:00"
        twins[1]["geometry"]["coordinates"] = [14.4, 50.05]
    bus = by_key(run(edited(edit)))["arriva-city-kladno-8996"]
    assert bus["location"]["value"]["coordinates"] == [14.4, 50.05]
    assert bus["dateObserved"]["value"] == "2026-10-22T18:10:30Z"


@requires_docker
def test_out_of_range_speed_heading_and_delay_are_left_out_and_the_position_kept():
    def edit(features):
        position = features[0]["properties"]["last_position"]
        position["speed"], position["bearing"], position["delay"]["actual"] = 400, 360, 99999
    tram = by_key(run(edited(edit)))["dp-praha-9099"]
    assert "location" in tram
    assert not {"speed", "heading", "delay"} & set(tram)


@requires_docker
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no PID vehicle"),
    ({"error": "Unauthorized"}, "not a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_prague(document, reason):
    entities, errors = outcome(document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


@requires_docker
def test_a_stale_answer_fails_the_run():
    entities, errors = outcome(RECORDED, now="2026-10-22T18:30:00Z")
    assert entities == [] and len(errors) == 1 and "older than 15 minutes" in errors[0], errors


def test_the_pipeline_reads_golemio_by_the_secret_once_a_minute_into_the_city_space():
    source = yaml.safe_load((PRAHA / "praha-datasource-pid-vehicles.yaml").read_text())
    http = source["spec"]["http"]
    assert http["url"].startswith("https://api.golemio.cz/v2/vehiclepositions")
    assert http["authorization"]["headerRef"] == {"name": "golemio", "key": "token"}
    assert "token" not in http["url"].lower()
    pipeline = yaml.safe_load((PRAHA / "praha-pipeline-pid-vehicles.yaml").read_text())
    assert pipeline["spec"]["period"] == "60s"
    assert pipeline["spec"]["output"] == {"type": "Vehicle", "mode": "upsert"}
    assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
