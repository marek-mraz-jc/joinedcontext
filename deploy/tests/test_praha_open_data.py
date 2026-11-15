"""Prague's open data in project `praha` (T-2785, PL-03, DM-01, DM-61, PF-84).

Bento runs each committed mapping over a recorded answer of the URL its DataSource declares,
fetched on 2026-09-25 and trimmed to a few records (`fixtures/praha/`), and every entity it writes
is checked against the space's generated JSON Schema, which the gateway enforces. The edge cases
are those same answers edited in the one place they name, never an invented feed.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import re
import shutil
import tempfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-open-data")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
FIXTURES = Path(__file__).resolve().parent / "fixtures/praha"
SPACE = "praha-mesto"

# pipeline -> (fixture, the type it writes, how many entities the recording holds)
FEEDS = {
    "bike-stations": ("bike-stations.json", "BikeHireDockingStation", 3),
    "bike-availability": ("bike-availability.json", "BikeHireDockingStation", 4),
    # Six rows: four registrations of Libuš, one of Legerova, one station elsewhere in the country.
    "air-quality": ("air-quality.csv", "AirQualityObserved", 2),
    "districts": ("districts.json", "AdministrativeArea", 2),
    "schools": ("schools.json", "PointOfInterest", 3),
    "culture": ("culture.json", "PointOfInterest", 2),
    "toilets": ("toilets.json", "PointOfInterest", 2),
    "waste-stations": ("waste-stations.json", "WasteContainerIsle", 2),
    "park-and-ride": ("park-and-ride.json", "OffStreetParking", 2),
    "ticket-points": ("ticket-points.json", "PointOfInterest", 3),
    # Three lines, one of them a correction the city files under no area.
    "budget": ("budget.json", "BudgetLine", 3),
    # Golemio (T-2907). Three car parks, one of them (TSK's b567b2eb) paired with no IPR record.
    "park-and-ride-occupancy": ("golemio-park-and-ride.json", "OffStreetParking", 2),
    # Two collection points, twelve containers, four of them monitored.
    "waste-fill": ("golemio-waste-containers.json", "WasteContainer", 4),
    # The same recording read whole (T-5427): the eight containers without a sensor.
    "waste-containers": ("golemio-waste-containers.json", "WasteContainer", 8),
}

# Mappings that answer one message per entity, which the runner batches.
SPLIT = {"waste-containers"}

# The memory a pipeline is given where 256 MiB is not enough, as measured in its manifest.
QUOTAS = {"waste-containers": 320}

# Feeds that only add live attributes to an entity another feed writes: its credit is that feed's,
# and the gateway's upsert merges, so writing a second credit would overwrite the first.
MERGED = {"park-and-ride-occupancy"}


def recorded(name: str) -> bytes:
    return (FIXTURES / name).read_bytes()


def run(pipeline: str, document: bytes) -> list[dict]:
    if pipeline == "waste-fill":
        # Its ask of the space needs the gateway: a failed ask writes every container.
        return waste_fill(document, "failed", [])
    if pipeline in SPLIT:
        return open_data.run_all(PRAHA / f"praha-pipeline-{pipeline}-bento.yaml", document, SPACE)
    return open_data.run(PRAHA / f"praha-pipeline-{pipeline}-bento.yaml", document, SPACE)


def schema_errors(entity: dict) -> list[str]:
    return open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity)


def edited(name: str, edit) -> bytes:
    document = json.loads(recorded(name).decode("utf-8-sig"))
    edit(document)
    return json.dumps(document).encode()


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name, recorded(fixture)) for name, (fixture, _, _) in FEEDS.items()}


@requires_docker
def test_every_feed_writes_its_one_type_and_every_record_it_can(written):
    for name, (_, kind, count) in FEEDS.items():
        assert {e["type"] for e in written[name]} == {kind}, name
        assert len(written[name]) == count, name


@requires_docker
def test_every_entity_is_one_the_gateway_accepts(written):
    for name, entities in written.items():
        for entity in entities:
            assert not schema_errors(entity), (name, entity["id"], schema_errors(entity))


@requires_docker
def test_every_id_is_the_four_segment_urn_of_the_space_and_unique_per_type(written):
    ids = {}
    for name, entities in written.items():
        for entity in entities:
            assert re.fullmatch(rf"urn:ngsi-ld:{entity['type']}:hel\.fi:{SPACE}:[a-z0-9-]+", entity["id"]), entity["id"]
            ids.setdefault(name, []).append(entity["id"])
    for name, own in ids.items():
        assert len(own) == len(set(own)), name


@requires_docker
def test_every_entity_credits_its_publisher_and_its_source(written):
    for name, entities in written.items():
        if name in MERGED:
            assert not any("dataProvider" in e or "source" in e for e in entities), name
            continue
        for entity in entities:
            assert entity["dataProvider"]["value"], entity["id"]
            assert entity["source"]["value"].startswith("https://"), entity["id"]
    ipr = [e for name in ("districts", "schools", "culture", "toilets", "waste-stations", "park-and-ride") for e in written[name]]
    # IPR's licence asks for this credit, word for word.
    assert all("datový podklad © IPR Praha" in e["dataProvider"]["value"] for e in ipr)


@requires_docker
def test_the_station_and_its_availability_are_one_entity(written):
    stations = {e["id"] for e in written["bike-stations"]}
    status = {e["id"]: e for e in written["bike-availability"]}
    assert stations <= set(status)
    # The fourth station of the recording neither rents nor takes back bikes.
    assert sorted(e["status"]["value"] for e in status.values()) == ["outOfService", "working", "working", "working"]
    assert all(e["dateModified"]["value"].endswith("Z") for e in status.values())


@requires_docker
def test_air_quality_keeps_prague_and_drops_a_missing_hour(written):
    libus = next(e for e in written["air-quality"] if e["id"].endswith(":chmi-alib"))
    assert {k for k in libus if k in ("pm10", "pm25", "no2", "o3", "so2", "co")} == {"pm10", "pm25", "no2", "o3"}
    assert libus["pm10"] == {"type": "Property", "value": 4.9, "unitCode": "GQ", "observedAt": "2026-09-25T03:00:00Z"}
    # Registration 10221 is outside Prague and wrote -5003, ČHMÚ's missing value; neither lands.
    assert "-5003" not in json.dumps(written["air-quality"])


@requires_docker
def test_a_negative_reading_is_never_written_as_a_value():
    text = recorded("air-quality.csv").decode().replace("40285, 2026-09-25T03:00:00Z, 8, 4.9", "40285, 2026-09-25T03:00:00Z, 6, -5003.0")
    libus = next(e for e in run("air-quality", text.encode()) if e["id"].endswith(":chmi-alib"))
    assert "pm10" not in libus and libus["pm25"]["value"] == 3.8


@requires_docker
def test_a_collection_point_points_at_its_district_by_ruian_code(written):
    point = written["waste-stations"][0]
    assert point["refDistrict"]["object"] == f"urn:ngsi-ld:AdministrativeArea:hel.fi:{SPACE}:district-538931"
    assert point["accessRestriction"]["value"] == "public"
    assert all(re.fullmatch(r"\d{6}", e["districtCode"]["value"]) for e in written["districts"])
    assert all(e["id"].endswith("district-" + e["districtCode"]["value"]) for e in written["districts"])


@requires_docker
def test_an_unknown_access_word_is_left_out_rather_than_guessed():
    def edit(d):
        d["features"][0]["properties"]["pristup"] = "po dohodě"
    first = run("waste-stations", edited("waste-stations.json", edit))[0]
    assert "accessRestriction" not in first and not schema_errors(first)


@requires_docker
def test_ids_follow_the_layer_globalid_not_its_objectid():
    def edit(d):
        d["features"][0]["properties"]["objectid"] = 999999
    again = run("schools", edited("schools.json", edit))
    assert [e["id"] for e in again] == [e["id"] for e in run("schools", recorded("schools.json"))]


@requires_docker
def test_a_row_without_a_name_or_a_key_is_dropped():
    def edit(d):
        d["features"][0]["properties"]["nazev_zar"] = "  "
        d["features"][1]["properties"]["globalid"] = None
    assert len(run("culture", edited("culture.json", edit))) == 0


@requires_docker
def test_a_web_field_that_is_no_single_address_leaves_the_place_without_its_url():
    """T-4011: the gateway holds `url` to an IRI, so `web` with two addresses or a space in one
    writes the place without a url instead of a place the gateway refuses."""
    def edit(d):
        d["features"][0]["properties"]["web"] = "www.divadlodisk.cz, www.disk.cz"
        d["features"][1]["properties"]["web"] = " www.divadlou22.cz "
    places = run("culture", edited("culture.json", edit))
    assert len(places) == 2
    assert "url" not in places[0] and not schema_errors(places[0])
    assert places[1]["url"]["value"] == "https://www.divadlou22.cz"


@requires_docker
def test_ticket_point_hours_read_as_day_ranges(written):
    hours = {e["id"].rsplit(":", 1)[1]: e["openingHours"]["languageMap"]["cs"] for e in written["ticket-points"]}
    assert hours["pid-dp2"] == "po–ne 5:00-24:00"
    assert hours["pid-cd551069"] == "po–so 05:50-19:05\nne 06:50-19:05"


@requires_docker
def test_budget_codes_keep_their_padding_and_the_byte_order_mark_is_read(written):
    lines = {e["id"].rsplit(":", 1)[1]: e for e in written["budget"]}
    school = lines["mhmp-2026-09-003122-6121-000000000"]
    assert school["budgetArea"]["value"] == "09 Školství"
    assert school["budgetFunction"]["value"] == "003122 Střední odborné školy"
    assert school["approvedAmount"]["value"] == 1700 and school["adjustedAmount"]["value"] == 102413.6
    correction = lines["mhmp-2026-00-000000-0000-000000000"]
    assert "budgetArea" not in correction and correction["adjustedAmount"]["value"] == -0.03
    # Each line reads what the money is for, the function and the item (T-4745).
    assert school["name"]["languageMap"] == {"cs": "Střední odborné školy: Stavby"}
    assert correction["name"]["languageMap"] == {"cs": "ČNB dotace OPP: neurčeno"}
    assert not schema_errors(school)
    # The key the id ends in is the identifier the BudgetLine extension (CPSV-AP Cost base) requires.
    assert all(e["identifier"]["value"] == key for key, e in lines.items())
    assert recorded("budget.json").startswith(b"\xef\xbb\xbf")


@requires_docker
def test_a_budget_line_with_an_amount_that_is_not_a_number_is_dropped():
    def edit(d):
        d["data"]["row"][0]["rozpocet_upraveny"] = "n/a"
    assert len(run("budget", edited("budget.json", edit))) == 2


@requires_docker
def test_an_empty_answer_writes_nothing():
    assert run("air-quality", recorded("air-quality.csv").splitlines()[0] + b"\n") == []
    assert run("ticket-points", b"[]") == []
    assert run("budget", b'{"data": {"row": []}}') == []
    for name in ("bike-stations", "bike-availability"):
        assert run(name, b'{"data": {"stations": []}}') == []
    for name in ("districts", "schools", "culture", "toilets", "waste-stations", "park-and-ride", "waste-fill"):
        assert run(name, b'{"type": "FeatureCollection", "features": []}') == [], name
    assert run("park-and-ride-occupancy", b"[]") == []


def test_a_value_outside_the_model_is_refused_by_the_schema():
    station = {
        "id": f"urn:ngsi-ld:BikeHireDockingStation:hel.fi:{SPACE}:nextbike-1",
        "type": "BikeHireDockingStation",
        "status": {"type": "Property", "value": "closed"},
    }
    assert any("closed" in message for message in schema_errors(station))
    station["status"]["value"] = "working"
    assert not schema_errors(station)
    station["availableBikeNumber"] = {"type": "Property", "value": -1}
    assert schema_errors(station)


@requires_docker
def test_occupancy_lands_on_the_ids_ipr_writes_and_an_unpaired_car_park_is_left_out(written):
    ids = {e["id"].rsplit(":", 1)[1] for e in written["park-and-ride-occupancy"]}
    # Skalka II and Kotlářka, by the globalid the park-and-ride pipeline keys them on.
    assert ids == {"ipr-a7c9a9af-a0fb-4005-8baf-c9a202b763ad", "ipr-8e869f78-c4e5-41fc-a316-ccff35083fdc"}
    skalka = next(e for e in written["park-and-ride-occupancy"] if e["id"].endswith("a7c9a9af-a0fb-4005-8baf-c9a202b763ad"))
    assert skalka["availableSpotNumber"]["value"] == 23 and skalka["occupiedSpotNumber"]["value"] == 55
    # The capacity comes with the free and occupied counts from the same feed (T-4105): IPR's register
    # gave P+R Nové Butovice 57 places while Golemio counted 112 free, a negative occupancy.
    assert skalka["totalSpotNumber"]["value"] == 78
    assert skalka["dateModified"]["value"] == "2026-09-25T13:34:01Z"
    # The name and the outline are IPR's; the live feed never writes them.
    assert not any(k in e for e in written["park-and-ride-occupancy"] for k in ("name", "location"))


@requires_docker
def test_a_count_that_is_not_a_whole_number_is_left_out_and_a_car_park_without_one_is_not_written():
    def edit(d):
        d[0]["free_spot_number"] = -3
        d[0]["occupied_spot_number"] = 12.5
        d[0]["total_spot_number"] = 78.5
        d[1]["free_spot_number"] = None
        d[1]["occupied_spot_number"] = "n/a"
        d[1]["total_spot_number"] = None
        d[1]["last_updated"] = None
    answer = run("park-and-ride-occupancy", edited("golemio-park-and-ride.json", edit))
    assert [set(e) - {"id", "type"} for e in answer] == [{"dateModified"}]
    assert not schema_errors(answer[0])


@requires_docker
def test_a_monitored_container_points_at_the_collection_point_the_ipr_feed_writes(written):
    isles = {e["stationCode"]["value"]: e["id"] for e in written["waste-stations"]}
    containers = written["waste-fill"]
    assert {c["refWasteContainerIsle"]["object"] for c in containers if "0022-001" in c["refWasteContainerIsle"]["object"]} == {isles["0022/ 001"]}
    metal = next(c for c in containers if c["id"].endswith(":ksnko-2"))
    assert metal["wasteKind"]["value"] == "metal" and metal["fillingLevel"]["value"] == 0.48
    assert metal["containerCode"]["value"] == "2" and metal["dateModified"]["value"] == "2026-09-25T10:35:39Z"
    # The station's address and the waste it takes name the container (T-4745).
    assert all(c["name"]["languageMap"]["cs"] for c in containers)
    assert not schema_errors(metal)


def waste_fill(document: bytes, asked: str, held: list[str]) -> list[dict]:
    """The waste-fill mapping with its ask of the space answered: `asked` is "ok" or "failed",
    `held` the collection points the space holds."""
    steps = yaml.safe_load((PRAHA / "praha-pipeline-waste-fill-bento.yaml").read_text())["pipeline"]["processors"]
    answer = {"mapping": f"meta asked = {json.dumps(asked)}\nmeta held = {json.dumps(' '.join(held))}"}
    steps = [answer if "branch" in step else step for step in steps]
    with tempfile.NamedTemporaryFile("w", suffix=".yaml") as mapping:
        yaml.safe_dump({"pipeline": {"processors": steps}}, mapping)
        mapping.flush()
        return open_data.run(Path(mapping.name), document, SPACE)


@requires_docker
def test_a_container_at_a_collection_point_the_space_does_not_hold_is_left_out(written):
    # T-5720: the view refuses a required link to an Entity it does not show (249 a run on dev).
    isles = sorted({c["refWasteContainerIsle"]["object"] for c in written["waste-fill"]})
    assert len(isles) == 2
    kept = waste_fill(recorded("golemio-waste-containers.json"), "ok", isles[:1])
    assert kept and {c["refWasteContainerIsle"]["object"] for c in kept} == {isles[0]}
    assert len(kept) < len(written["waste-fill"])


@requires_docker
def test_a_failed_or_empty_ask_writes_every_container(written):
    isles = sorted({c["refWasteContainerIsle"]["object"] for c in written["waste-fill"]})
    # `written` is the failed ask; an empty answer and one holding every point write the same.
    def timeless(entities):
        return [{k: v for k, v in e.items() if k != "expiresAt"} for e in entities]
    for held in ([], isles):
        assert timeless(waste_fill(recorded("golemio-waste-containers.json"), "ok", held)) == timeless(written["waste-fill"]), held


@requires_docker
def test_a_container_expires_a_week_after_its_last_write_so_one_golemio_stops_monitoring_leaves(written):
    # T-5270: 199 containers absent from Golemio's answer kept a fill level from up to four weeks back.
    from datetime import UTC, datetime, timedelta
    now = datetime.now(UTC)
    for container in written["waste-fill"]:
        expires = datetime.fromisoformat(container["expiresAt"])
        assert now + timedelta(days=6, hours=23) <= expires <= now + timedelta(days=7, minutes=5), container["expiresAt"]


@requires_docker
def test_an_unmonitored_or_unknown_container_is_not_written_and_a_missing_reading_writes_no_level():
    def edit(d):
        monitored = [c for c in d["features"][0]["properties"]["containers"] if c["is_monitored"]]
        monitored[0]["trash_type"] = {"id": 0, "description": "neznámý"}
        monitored[1]["last_measurement"]["percent_calculated"] = None
        monitored[2]["last_measurement"]["percent_calculated"] = 140
    answer = run("waste-fill", edited("golemio-waste-containers.json", edit))
    assert len(answer) == 3
    assert sum("fillingLevel" in c for c in answer) == 1
    assert all(not schema_errors(c) for c in answer)


@requires_docker
def test_the_two_spellings_of_a_register_number_are_two_collection_points():
    def edit(d):
        d["features"][1]["properties"]["stationnumber"] = d["features"][0]["properties"]["stationnumber"].replace("/ ", "/-")
    ids = [e["id"].rsplit(":", 1)[1] for e in run("waste-stations", edited("waste-stations.json", edit))]
    # `0022/ 001` and `0022/-001` are two points of the register (791 such pairs on 2026-09-25).
    assert ids == ["station-0022-001", "station-0022--001"]


def waste_containers(document: bytes) -> tuple[list[dict], list[str]]:
    """The waste-containers mapping over one answer: the entities written and the error of a message
    the runner refuses (an errored message fails the run, nothing is written)."""
    answer = open_data.run_all(PRAHA / "praha-pipeline-waste-containers-bento.yaml", document, SPACE,
                               then=({"mapping": 'root = if errored() { {"__error": error()} } else { this }'},))
    errors = [e["__error"] for e in answer if "__error" in e]
    return ([], errors) if errors else (answer, [])


@requires_docker
def test_every_container_without_a_sensor_is_written_with_its_next_collection_at_its_point(written):
    recording = json.loads(recorded("golemio-waste-containers.json"))
    unmonitored = {str(c["ksnko_id"]) for f in recording["features"] for c in f["properties"]["containers"] if not c["is_monitored"]}
    containers = {e["id"].rsplit(":ksnko-", 1)[1]: e for e in written["waste-containers"]}
    assert set(containers) == unmonitored
    # The sensor containers stay waste-fill's: the two pipelines never write one entity.
    assert not {e["id"] for e in written["waste-fill"]} & {e["id"] for e in written["waste-containers"]}
    isles = {e["stationCode"]["value"]: e["id"] for e in written["waste-stations"]}
    paper = containers["4"]
    assert paper["refWasteContainerIsle"]["object"] == isles["0022/ 001"]
    assert paper["wasteKind"]["value"] == "paper" and paper["containerCode"]["value"] == "4"
    assert paper["name"]["languageMap"]["cs"] == "Přátelství 356/61, Papír"
    # next_pick 2026-09-27: collected during that day in Prague, summer time.
    assert paper["nextActuationDeadline"]["value"] == "2026-09-27T23:59:59+02:00"
    assert paper["location"]["value"] == {"type": "Point", "coordinates": recording["features"][0]["geometry"]["coordinates"]}
    # The edible-oil container has no collection day in the register: it lands without one.
    assert "nextActuationDeadline" not in containers["28175"]
    # No reading time, so the collection desk (praha-odpad, q=fillingLevel|dateModified) never lists them.
    assert not any("fillingLevel" in e or "dateModified" in e for e in containers.values())
    for entity in containers.values():
        assert not schema_errors(entity), schema_errors(entity)
        assert not open_data.sdm_errors("dataModel.WasteManagement/WasteContainer", entity), entity["id"]


@requires_docker
def test_an_out_of_area_point_an_unknown_kind_and_a_duplicate_are_dropped_a_bad_collection_day_left_out():
    def edit(d):
        first, second = d["features"]
        containers = first["properties"]["containers"]
        containers[0]["trash_type"] = {"id": 0, "description": "neznámý"}
        containers[1]["cleaning_frequency"]["next_pick"] = "2026-13-45"
        containers.append(dict(containers[2]))
        second["geometry"]["coordinates"] = [16.6068, 49.1951]
    answer, errors = waste_containers(edited("golemio-waste-containers.json", edit))
    assert not errors, errors
    ids = [e["id"].rsplit(":ksnko-", 1)[1] for e in answer]
    # 4 unknown kind, 3516 and 37541 stand in Brno, 39586 listed twice is written once.
    assert sorted(ids) == sorted(["5", "39586", "39587", "28175", "21382"])
    five = next(e for e in answer if e["id"].endswith(":ksnko-5"))
    assert "nextActuationDeadline" not in five and not schema_errors(five)


@requires_docker
def test_a_list_where_fewer_than_half_survive_fails_the_run_instead_of_thinning_the_containers():
    def edit(d):
        for feature in d["features"]:
            feature["geometry"]["coordinates"] = [0, 0]
    answer, errors = waste_containers(edited("golemio-waste-containers.json", edit))
    assert answer == [] and len(errors) == 1 and "only 0 of 8" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("document,reason", [
    (b'{"type": "FeatureCollection", "features": []}', "listed no collection point"),
    (b'{"error_message": "Unauthorized"}', "did not answer a feature list"),
])
def test_an_empty_or_broken_container_list_fails_the_run(document, reason):
    answer, errors = waste_containers(document)
    assert answer == [] and len(errors) == 1 and reason in errors[0], errors


def test_the_golemio_key_is_named_by_reference_and_committed_only_encrypted():
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    for name in ("park-and-ride-occupancy", "waste-fill", "waste-containers"):
        http = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())["spec"]["http"]
        assert http["authorization"] == {"header": "X-Access-Token", "headerRef": {"name": "golemio", "key": "token"}}
        assert not any(h.lower() in ("x-access-token", "authorization") for h in http["headers"])
        assert http["url"].startswith("https://api.golemio.cz/") and "token" not in http["url"].lower()
    # The encrypted key itself is private: an environment adds it (encryptedSeedFiles, T-6982;
    # the private environments/tests, T-6984), so no seed folder holds or names a sops file.
    seeds = PRAHA.parent
    assert not sorted(seeds.glob("**/*.enc.y*ml"))
    assert not [f for f in index if f.endswith((".enc.yaml", ".enc.yml"))]


def test_every_feed_is_seeded_as_a_datasource_a_pipeline_and_a_mapping():
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    model = yaml.safe_load((PRAHA / "praha-datamodel.yaml").read_text())
    public = yaml.safe_load((PRAHA / "praha-policy-public-read.yaml").read_text())
    readable = {e["type"] for rule in public["spec"]["information"] for e in rule["entities"]}
    for name, (_, kind, _) in FEEDS.items():
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"]
        assert pipeline["spec"]["output"]["type"] == kind
        assert pipeline["spec"]["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:praha-mesto:praha-mesto"
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= QUOTAS.get(name, 256), name
        assert datasource["spec"]["http"]["url"].startswith("https://")
        assert kind in model["spec"]["classes"] and kind in readable
        for seeded in (f"praha-datasource-{name}.yaml", f"praha-pipeline-{name}.yaml", f"praha-pipeline-{name}-bento.yaml"):
            assert seeded in index, seeded


def test_every_pipeline_is_scheduled_and_the_project_has_room_for_a_hundred_more():
    """T-2873: the praha pipelines were refused on dev by a resident quota of one."""
    project = yaml.safe_load((PRAHA / "praha-project.yaml").read_text())
    assert project["spec"]["quotas"]["residentPipelines"] >= 100
    for name in FEEDS:
        period = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())["spec"]["period"]
        seconds = int(period[:-1]) * {"s": 1, "m": 60, "h": 3600}[period[-1]]
        # PL-26: 30 s or longer is scheduled.
        assert seconds >= 300, name


def test_the_seed_index_lists_every_file_beside_it_and_the_forge_seeds_it():
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    assert set(index) == {p.name for p in PRAHA.iterdir()} - {"index.yaml"}
    assert all(path.startswith("projects/praha/") for path in index.values())
    bootstrap = (ROOT / "components/gitea/values/bootstrap/development-values.yaml.gotmpl").read_text()
    assert 'context-gateway/seed/praha/index.yaml' in bootstrap


# The space's datasets, one Smart Data Model type or one referencing group each (T-4344), with the
# licence of the most demanding source each reads: nextbike and the city budget are CC0.
PARTS = {
    "praha-ovzdusi": "CC_BY_4_0", "praha-sdilena-kola": "CC0", "praha-parkoviste-pr": "CC_BY_4_0",
    "praha-trideny-odpad": "CC_BY_4_0", "praha-rozpocet": "CC0", "praha-mista": "CC_BY_4_0",
    "praha-mestske-casti": "CC_BY_4_0",
    # Golemio's terms allow republishing but grant no sublicence: no licence of our own is claimed
    # (owner 2026-10-22T18:40Z, T-5268).
    "praha-vybavenost": "CC_BY_4_0",
    "praha-zdravotni-a-socialni-sluzby": "CC_BY_4_0",
    "praha-zdroje-znecisteni": "CC_BY_4_0",
    "praha-radon": "CC_BY_4_0",
    "praha-zaplavova-uzemi": "CC_BY_4_0",
    "praha-hluk": "CC_BY_4_0",
    "praha-protipovodnova-ochrana": "CC_BY_4_0",
    "praha-pocasi": "CC_BY_4_0",
    # The PID network, one dataset per type (T-5423).
    "praha-linky-pid": "CC_BY_4_0", "praha-zastavky-pid": "CC_BY_4_0", "praha-vstupy-metra": "CC_BY_4_0",
    "praha-tarifni-pasma": "CC_BY_4_0",
    # The IPR place layers drawn as areas or lines, one dataset per layer (T-5426).
    "praha-hrbitovy": "CC_BY_4_0", "praha-naucne-stezky": "CC_BY_4_0", "praha-vodni-plochy": "CC_BY_4_0",
    "praha-psi-zony": "CC_BY_4_0", "praha-bilancni-celky": "CC_BY_4_0", "praha-cyklotrasy": "CC_BY_4_0",
    # The IPR layers without a name field (T-5682).
    "praha-pietni-mista": "CC_BY_4_0", "praha-posty": "CC_BY_4_0", "praha-technicke-prvky-zelene": "CC_BY_4_0",
    "praha-zelen": "CC_BY_4_0",
    "praha-stromy": "CC_BY_4_0",
    "praha-parkovani": "CC_BY_4_0",
    "praha-zakazy-stani": "CC_BY_4_0",
    # TSK's street sections and summer street cleaning dates as RoadSegment (T-5677).
    "praha-ulicni-useky": "CC_BY_4_0", "praha-letni-udrzba": "CC_BY_4_0",
    "praha-parkovaci-automaty": "CC_BY_4_0",
    # The city tables and road works, one dataset per file family; the city and its police waive
    # every right (T-5428).
    "praha-cinnost-mestske-policie": "CC0", "praha-cinnost-mestske-policie-utvary": "CC0",
    "praha-cerpani-rozpoctu": "CC0", "praha-financni-vztahy-mc": "CC0", "praha-opravy-a-uzavirky": "CC0",
    # Golemio's Mr.Parkit garages and shared cars (T-5680) left the catalogue while empty on dev (T-5767).
}


def _parts():
    docs = [yaml.safe_load(p.read_text()) for p in PRAHA.glob("praha-endpoint-*.yaml")]
    # The city's own catalogue (T-5493) publishes too, from its own space: no part of this one.
    # Prague's events (T-5666) are a space of their own, praha-akce: no part of this one either.
    return {d["metadata"]["name"]: d for d in docs if d["spec"].get("publish") and d["metadata"]["name"] != "catalog"
            and d["spec"]["contextSpaceRef"] == SPACE}


def test_the_space_is_public_and_published_one_model_per_dataset_with_its_sources_licence():
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-mesto.yaml").read_text())
    space = yaml.safe_load((PRAHA / "praha-space-mesto.yaml").read_text())
    assert endpoint["spec"]["audience"] == "public"
    # The whole space stays one Endpoint for the Apps, off the catalogue (T-4344).
    assert "publish" not in endpoint["spec"]
    parts = _parts()
    assert {n: d["spec"]["catalog"].get("license") for n, d in parts.items()} == PARTS
    for name, part in parts.items():
        assert part["spec"]["audience"] == "public", name
        assert part["spec"]["publish"]["ckan"]["instanceRef"]["name"] == "praha", name
        assert part["spec"]["policyRef"].startswith(f"urn:ngsi-ld:Policy:praha.eu:{SPACE}:"), name
    assert space["spec"]["urnSegment"] == SPACE
    for publisher in ("nextbike", "ČHMÚ", "IPR Praha", "ROPID"):
        assert publisher in space["metadata"]["description"]["en"]


def test_the_model_classes_and_the_manifest_agree():
    linkml = yaml.safe_load((PRAHA / "praha.linkml.yaml").read_text())
    manifest = yaml.safe_load((PRAHA / "praha-datamodel.yaml").read_text())
    # A value class such as PostalAddress (T-4329) is the shape of one attribute, not a type the manifest lists.
    assert set(manifest["spec"]["classes"]) == {c for c, body in linkml["classes"].items() if body.get("is_a") == "Entity"}


def _dataset_of(url):
    """The original dataset a DataSource reads: a GBFS feed belongs to its system's `gbfs.json`."""
    gbfs = re.match(r"^(.*)/[a-z]{2}/station_(?:status|information)\.json$", url)
    return f"{gbfs.group(1)}/gbfs.json" if gbfs else url


def test_the_catalogue_names_every_source_the_space_reads_under_the_licence_it_publishes():
    """T-2792, EP-78, EP-79: the DCAT-AP block names each original dataset the pipelines read,
    so a feed added to the space without its source in the record is red; the licence is the one
    the CKAN publication carries, the words in Czech and English, the place Prague."""
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-mesto.yaml").read_text())
    catalog = endpoint["spec"]["catalog"]
    assert catalog["license"] == "CC_BY_4_0"
    named = {source["url"] for source in catalog["source"]}
    # The datasets on the catalogue name every source between them, each as the space does.
    by_url = {source["url"]: source for source in catalog["source"]}
    # A part off the catalogue while empty (harvest: false, T-5767) still names its source in its own record.
    parts = list(_parts().values()) + [d for d in (yaml.safe_load(p.read_text()) for p in PRAHA.glob("praha-endpoint-*.yaml"))
                                       if d["spec"]["contextSpaceRef"] == SPACE and d["spec"].get("catalog", {}).get("harvest") is False]
    parted = [s for part in parts for s in part["spec"]["catalog"].get("source", [])]
    assert {s["url"] for s in parted} == named
    for source in parted:
        assert source == by_url[source["url"]], source["url"]
    for name in FEEDS:
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert _dataset_of(datasource["spec"]["http"]["url"]) in named, name
    # The events sources (seeds/bbsk-events/generate.py, T-5666) feed praha-akce, whose Endpoint lists them.
    assert named == {
        _dataset_of(yaml.safe_load(p.read_text())["spec"]["http"]["url"]) for p in PRAHA.glob("praha-datasource-*.yaml")
        if not p.name.startswith(("praha-datasource-podujatia-", "praha-datasource-miesta-podujati"))
    }, "a source the space does not read"
    for source in catalog["source"]:
        assert set(source["title"]) == set(source["description"]) == {"cs", "en"}, source["url"]
    assert set(catalog["keywords"]) == set(catalog["attribution"]) == {"cs", "en"}
    # The publisher is the Project's, by its RPP IRI (T-4596): the record names none of its own.
    assert "publisher" not in catalog
    assert len(catalog["keywords"]["cs"]) == len(catalog["keywords"]["en"])
    assert "CZ010" in catalog["spatial"]
    # A contact is a role address the owner chooses, never a person guessed here (EP-80).
    assert "contactPoint" not in catalog


# T-4329: each Prague type and the vendored Smart Data Model it is.
SDM_MODELS = {
    "BikeHireDockingStation": "dataModel.Transportation/BikeHireDockingStation",
    "AirQualityObserved": "dataModel.Environment/AirQualityObserved",
    "PointOfInterest": "dataModel.PointOfInterest/PointOfInterest",
    "WasteContainerIsle": "dataModel.WasteManagement/WasteContainerIsle",
    "WasteContainer": "dataModel.WasteManagement/WasteContainer",
    "OffStreetParking": "dataModel.Parking/OffStreetParking",
    "Garden": "dataModel.ParksAndGardens/Garden",
    # T-4338: no official model holds these as they are; each is the extension of its nearest one.
    "AdministrativeArea": "extensions/AdministrativeArea",
    "BudgetLine": "extensions/BudgetLine",
}


@requires_docker
def test_every_entity_a_feed_writes_whole_is_valid_in_its_smart_data_model(written):
    """A merge-only feed adds attributes to an entity another feed writes whole, so only the whole
    writes are held to the model; an address is the model's PostalAddress, a container stands
    where its collection point is."""
    for name, entities in written.items():
        if name in MERGED:
            continue
        for entity in entities:
            model = SDM_MODELS.get(entity["type"])
            if model:
                assert open_data.sdm_errors(model, entity) == [], (name, entity["id"])
    toilet = written["toilets"][0]["address"]["value"]
    assert toilet["addressLocality"] == "Praha" and toilet["addressCountry"] == "CZ" and toilet["streetAddress"].strip()
