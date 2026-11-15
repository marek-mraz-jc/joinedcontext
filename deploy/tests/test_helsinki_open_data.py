"""The city's open registers in the Helsinki space (T-2787, T-3356, PL-03, DM-01, DM-61).

Bento runs each committed mapping over a recorded answer of the URL its DataSource declares,
fetched on 2026-09-25 and trimmed to a few records (`fixtures/helsinki/open-data/`), and every
entity it writes is checked against the space's generated JSON Schema: the model is what the
gateway enforces, so an entity the schema refuses is a write the gateway would refuse. The edge
cases are those same answers edited in the one place they name, never an invented feed.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
FIXTURES = Path(__file__).resolve().parent / "fixtures/helsinki/open-data"

# pipeline -> (fixture, the type it writes, how many entities the recording holds)
FEEDS = {
    "services": ("servicemap-units.json", "PointOfInterest", 8),
    "beach-water": ("uiras-beaches.json", "WaterQualityObserved", 3),
    "parking-areas": ("parking-areas.json", "OnStreetParking", 3),
    # Three areas, two of them one application: two permits.
    "excavation": ("excavation.json", "CityWork", 2),
    "traffic-arrangements": ("traffic-arrangements.json", "CityWork", 2),
    "area-events": ("area-events.json", "CityWork", 3),
    # Two features, one of them the territorial sea, which has no district number.
    "districts": ("districts.json", "AdministrativeArea", 1),
    "subdistricts": ("subdistricts.json", "AdministrativeArea", 2),
    # T-5256: the three smallest outlines of each layer, fetched 2026-10-26; one quarter carries a name of its own.
    "voting-districts": ("voting-districts.json", "AdministrativeArea", 3),
    "quarters": ("quarters.json", "AdministrativeArea", 3),
    # T-5822, fetched 2026-10-27: a plot with numbers, one only ships may use (no floor area), one whose area is worded;
    # a permit site with a full address, one at the city's own postal code, one with no address at all;
    # the smallest Finnish and Swedish area of 2026-2027 and the smallest Finnish one of 2025-2026.
    "business-plots": ("business-plots.json", "PointOfInterest", 3),
    "environmental-permit-sites": ("environmental-permits.json", "PointOfInterest", 3),
    "preschool-areas": ("preschool-areas.json", "AdministrativeArea", 3),
    "fee-parking-zones": ("payment-zones.json", "OnStreetParking", 1),
    "resident-parking-zones": ("resident-zones.json", "OnStreetParking", 2),
    # Every row of three line variants: a ferry, a metro line and a one-stop variant.
    "transit-routes": ("hsl-routes.json", "PublicTransportRoute", 3),
}


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text())


def run(pipeline: str, document: dict) -> list[dict]:
    if pipeline == "services":
        return services(document)
    if pipeline == "preschool-areas":
        return preschool(document, [PRESCHOOL_UNIT.format(148)])
    return open_data.run_all(HELSINKI / f"helsinki-pipeline-{pipeline}-bento.yaml", json.dumps(document).encode(), "helsinki")


SERVICE_TREE = "api.hel.fi/servicemap/v2/service_node/"


def services(document: dict, tree: dict | None = None) -> list[dict]:
    """The services mapping with its two asks of the service tree answered by `tree` (by default the
    recorded part of the tree that holds every node the recorded units name, fetched 2026-10-25).
    A message the runner would refuse (an errored one) comes back as {"__error": ...}."""
    tree = fixture("servicemap-tree.json") if tree is None else tree
    answer = {"mapping": f"root = this.assign({{\"tree\": {json.dumps(tree['results'])}, \"tree_count\": {tree['count']}}})"}
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-services-bento.yaml").read_text())["pipeline"]["processors"]
    asks = [i for i, step in enumerate(steps) if SERVICE_TREE in json.dumps(step)]
    assert len(asks) == 2, asks
    steps = steps[:asks[0]] + [answer] + [step for i, step in enumerate(steps) if i > asks[0] and i not in asks]
    steps.append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    with tempfile.NamedTemporaryFile("w", suffix=".yaml") as mapping:
        yaml.safe_dump({"pipeline": {"processors": steps}}, mapping)
        mapping.flush()
        return open_data.run_all(Path(mapping.name), json.dumps(document).encode(), "helsinki")


def schema_errors(entity: dict) -> list[str]:
    return open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity)


BEACH_UNIT = "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-{}"


def beach_steps() -> list[dict]:
    return yaml.safe_load((HELSINKI / "helsinki-pipeline-beach-water-bento.yaml").read_text())["pipeline"]["processors"]


def beach(document: dict, held: list[str]) -> list[dict]:
    """The beach-water mapping with its ask of the space answered by `held`, the units it holds
    (an empty list is also what a failed ask leaves)."""
    steps = [{"mapping": f"meta held = {json.dumps(' '.join(held))}"} if "branch" in p else p for p in beach_steps()]
    with tempfile.NamedTemporaryFile("w", suffix=".yaml") as mapping:
        yaml.safe_dump({"pipeline": {"processors": steps}}, mapping)
        mapping.flush()
        return open_data.run_all(Path(mapping.name), json.dumps(document).encode(), "helsinki")


PRESCHOOL_UNIT = "urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-{}"


def preschool(document: dict, held: list[str]) -> list[dict]:
    """The preschool-areas mapping with its ask of the space answered by `held`, the units it holds
    (an empty list is also what a failed ask leaves)."""
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-preschool-areas-bento.yaml").read_text())["pipeline"]["processors"]
    assert sum("branch" in p for p in steps) == 1
    steps = [{"mapping": f"meta held = {json.dumps(' '.join(held))}"} if "branch" in p else p for p in steps]
    with tempfile.NamedTemporaryFile("w", suffix=".yaml") as mapping:
        yaml.safe_dump({"pipeline": {"processors": steps}}, mapping)
        mapping.flush()
        return open_data.run_all(Path(mapping.name), json.dumps(document).encode(), "helsinki")


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    feeds = {name: run(name, fixture(recording)) for name, (recording, _, _) in FEEDS.items() if name != "beach-water"}
    # The services recording holds units 41960 and 40157; 40356 is a swimming place (service 730) it never writes.
    return {**feeds, "beach-water": beach(fixture("uiras-beaches.json"), [BEACH_UNIT.format(41960), BEACH_UNIT.format(40157)])}


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
def test_every_id_is_the_four_segment_urn_of_the_space_and_unique(written):
    ids = [e["id"] for entities in written.values() for e in entities]
    assert len(ids) == len(set(ids))
    for entity in (e for entities in written.values() for e in entities):
        assert re.fullmatch(rf"urn:ngsi-ld:{entity['type']}:hel\.fi:helsinki:[A-Za-z0-9-]+", entity["id"]), entity["id"]


@requires_docker
def test_every_entity_credits_its_publisher_and_its_source(written):
    for entities in written.values():
        for entity in entities:
            assert entity["dataProvider"]["value"], entity["id"]
            assert entity["source"]["value"].startswith("https://"), entity["id"]


@requires_docker
def test_the_areas_of_one_application_are_one_permit_with_every_outline(written):
    areas = fixture("excavation.json")["features"]
    numbers = [a["properties"]["hakemustunnus"] for a in areas]
    shared = next(n for n in numbers if numbers.count(n) == 2)
    permit = next(e for e in written["excavation"] if e["permitNumber"]["value"] == shared)
    assert permit["id"] == f"urn:ngsi-ld:CityWork:hel.fi:helsinki:{shared}"
    assert permit["location"]["value"]["type"] == "MultiPolygon"
    assert len(permit["location"]["value"]["coordinates"]) == 2
    # "34 PAKILA" names district 34, which the districts pipeline writes as district-34.
    district = next(a for a in areas if a["properties"]["hakemustunnus"] == shared)["properties"]["kaupunginosa"]
    number = int(district.split()[0])
    assert permit["refDistrict"] == {
        "type": "Relationship", "object": f"urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:district-{number}",
    }
    # The register names none of these permits: each reads its address instead (T-4745).
    assert permit["name"]["languageMap"]["fi"] == "Rapparinkuja 8b"
    assert all(e["name"]["languageMap"]["fi"] for e in written["excavation"])


@requires_docker
@pytest.mark.parametrize("feed", ["excavation", "traffic-arrangements", "area-events"])
def test_a_permit_expires_when_its_end_day_ends_or_a_day_after_the_layer_last_listed_it(written, feed):
    """T-5167: a permit the layer dropped stayed in the space with its last state; the catalogue sampled
    such rows. Each run moves the lease a day on, and an ended permit expires at the end of its day."""
    from datetime import datetime, timedelta, timezone

    now = datetime.now(timezone.utc)
    for permit in written[feed]:
        expires = datetime.strptime(permit["expiresAt"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
        end = datetime.strptime(permit["permitEnd"]["value"] + "T23:59:59Z", "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
        assert expires == min(end, expires) and expires <= now + timedelta(days=1, minutes=5), permit["id"]
        assert expires == end or expires >= now + timedelta(days=1, minutes=-5), permit["id"]


@requires_docker
def test_an_event_without_build_dates_is_covered_by_its_own_days(written):
    events = {e["permitNumber"]["value"]: e for e in written["area-events"]}
    for area in fixture("area-events.json")["features"]:
        props = area["properties"]
        permit = events[props["hakemustunnus"]]
        expected = props["rakentaminen_alkaa"] if re.fullmatch(r"\d{4}-\d\d-\d\d", str(props["rakentaminen_alkaa"])) else props["tapahtuma_alkaa"]
        assert permit["permitStart"]["value"] == expected


@requires_docker
def test_no_permit_carries_its_applicant_or_contractor_even_when_the_register_fills_them():
    recording = fixture("excavation.json")
    for area in recording["features"]:
        area["properties"]["hakija"] = "Applicant Person"
        area["properties"]["tyon_suorittaja"] = "Contractor Person"
    for permit in run("excavation", recording):
        assert "Person" not in json.dumps(permit)


@requires_docker
def test_a_permit_the_register_gives_no_status_or_no_dates_is_dropped():
    recording = fixture("area-events.json")
    first, second = recording["features"][0], recording["features"][1]
    first["properties"]["status"] = "Peruttu"
    second["properties"]["tapahtuma_alkaa"] = "None"
    second["properties"]["rakentaminen_alkaa"] = "None"
    kept = {e["permitNumber"]["value"] for e in run("area-events", recording)}
    assert first["properties"]["hakemustunnus"] not in kept
    assert second["properties"]["hakemustunnus"] not in kept
    assert len(kept) == 1


@requires_docker
def test_a_unit_writes_its_opening_hours_and_never_its_contact_person():
    recording = fixture("servicemap-units.json")
    library = recording["results"][0]
    library["connections"].append({
        "section_type": "PHONE_OR_EMAIL", "name": {"fi": "Asiakaspalvelu"},
        "phone": "+358 9 000 0000", "email": "someone@example.test", "contact_person": "Staff Member",
    })
    entity = next(e for e in run("services", recording) if e["id"].endswith(f"servicemap-{library['id']}"))
    assert entity["serviceCategory"]["value"] == "library"
    assert entity["openingHours"]["languageMap"]["fi"].startswith("Normaaliaukioloaika")
    text = json.dumps(entity)
    assert "Staff Member" not in text and "someone@example.test" not in text and "+358" not in text


@requires_docker
def test_a_unit_of_any_other_service_is_written_as_other():
    """T-5257: every unit of the register, so each Linked Events venue names a PointOfInterest."""
    recording = fixture("servicemap-units.json")
    recording["results"][0]["services"] = [9999]
    recording["results"][0]["service_nodes"] = []
    written = run("services", recording)
    assert len(written) == 8
    other = next(e for e in written if e["id"].endswith(f"servicemap-{recording['results'][0]['id']}"))
    assert other["serviceCategory"]["value"] == "other" and not schema_errors(other)


@requires_docker
def test_a_unit_point_outside_helsinki_is_left_out_and_a_twice_listed_unit_written_once():
    recording = fixture("servicemap-units.json")
    first, second = recording["results"][0], recording["results"][1]
    first["location"] = {"type": "Point", "coordinates": [60.17, 24.94]}  # swapped
    recording["results"].append(json.loads(json.dumps(second)))
    written = {e["id"]: e for e in run("services", recording)}
    assert len(written) == 8
    assert "location" not in written[f"urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-{first['id']}"]


@requires_docker
def test_an_empty_register_fails_the_run_and_the_pages_after_the_first_are_asked_for():
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-services-bento.yaml").read_text())["pipeline"]["processors"]
    http = next(s for s in steps if "branch" in s and "/unit/" in json.dumps(s))["branch"]["processors"][-1]["http"]
    assert http["url"].startswith("https://api.hel.fi/servicemap/v2/unit/?municipality=helsinki&page_size=1000&")
    assert http["url"].endswith("&page=${! @page }")
    # The thrown mapping error flags the message: the runner refuses it and nothing reaches the endpoint.
    refused = run("services", {"count": 0, "results": []})
    assert len(refused) == 1 and "the Service Map lists no unit in Helsinki" in refused[0]["__error"]


def unit_entity(written: list[dict], unit: int) -> dict:
    return next(e for e in written if e["id"] == f"urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-{unit}")


@requires_docker
def test_a_public_institution_takes_its_kind_from_the_service_tree_and_its_point_from_its_record(written):
    """T-5689: Haartman hospital's cardiology clinic (a joint municipal authority, node 1009 Hospitals)
    and a city playpark (node 868 Day care); the five older kinds keep their service codes, the
    private swimming hall Kellumo included."""
    kinds = {int(e["id"].rsplit("-", 1)[1]): e["serviceCategory"]["value"] for e in written["services"]}
    assert kinds == {10037: "library", 6247: "healthStation", 80486: "swimmingHall", 62953: "beach", 7010: "school",
                     80752: "hospital", 80964: "kindergarten", 81019: "other"}
    for unit in (80752, 80964):
        entity = unit_entity(written["services"], unit)
        assert entity["location"]["value"]["type"] == "Point"
        assert entity["source"]["value"] == f"https://api.hel.fi/servicemap/v2/unit/{unit}/"
        assert not schema_errors(entity)


@requires_docker
def test_a_private_unit_or_one_without_a_point_in_the_city_never_takes_an_institution_kind():
    recording = fixture("servicemap-units.json")
    hospital, playpark, coworking = (next(u for u in recording["results"] if u["id"] == i) for i in (80752, 80964, 81019))
    hospital["contract_type"] = {"id": "PRIVATE_SERVICE"}
    del playpark["location"]
    coworking["contract_type"] = {"id": "MUNICIPAL_SERVICE"}
    coworking["location"]["coordinates"] = [24.66, 60.2]  # Espoo, west of the city
    written = services(recording)
    assert {unit_entity(written, i)["serviceCategory"]["value"] for i in (80752, 80964, 81019)} == {"other"}
    assert all(not schema_errors(e) for e in written)


@requires_docker
def test_a_public_unit_of_a_health_or_social_service_with_no_kind_above_is_health_care_or_social_services():
    recording = fixture("servicemap-units.json")
    hospital = next(u for u in recording["results"] if u["id"] == 80752)
    hospital["service_nodes"] = []
    assert unit_entity(services(recording), 80752)["serviceCategory"]["value"] == "healthCare"
    hospital["root_service_nodes"] = [783]
    assert unit_entity(services(recording), 80752)["serviceCategory"]["value"] == "socialServices"


@requires_docker
def test_a_service_tree_the_two_pages_do_not_hold_fails_the_run():
    tree = fixture("servicemap-tree.json")
    refused = services(fixture("servicemap-units.json"), {**tree, "count": 2001})
    assert len(refused) == 1 and "the service tree lists 2001 nodes and 68 were read" in refused[0]["__error"]
    assert len(services(fixture("servicemap-units.json"), {"count": 0, "results": []})) == 1


def test_the_service_tree_is_asked_page_by_page_with_a_size_cap():
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-services-bento.yaml").read_text())["pipeline"]["processors"]
    asks = [step["branch"] for step in steps if SERVICE_TREE in json.dumps(step)]
    assert [a["processors"][-1]["http"]["url"][-7:] for a in asks] == ["&page=1", "&page=2"]
    for ask in asks:
        http = ask["processors"][-1]["http"]
        assert http["url"].startswith("https://api.hel.fi/servicemap/v2/service_node/?page_size=1000&only=id,parent,level&")
        assert http["timeout"] == "60s" and "4194304" in ask["result_map"]
    # The second page is asked only when the first says there is one.
    assert "this.tree_count.or(0) > 1000" in asks[1]["request_map"]


@requires_docker
def test_a_beach_sensor_without_a_reading_has_no_temperature_and_points_at_its_beach(written):
    silent = [e for e in written["beach-water"] if "temperature" not in e]
    assert len(silent) == 1 and "dateObserved" not in silent[0]
    reading = next(e for e in written["beach-water"] if "temperature" in e)
    assert reading["temperature"]["unitCode"] == "CEL"
    assert reading["temperature"]["observedAt"].endswith("Z")
    assert reading["refPointOfInterest"]["object"].startswith("urn:ngsi-ld:PointOfInterest:hel.fi:helsinki:servicemap-")


@requires_docker
def test_a_beach_sensor_links_only_a_unit_the_space_holds(written):
    """T-5345: the gateway refuses a link to an entity the space does not hold (DM-70); 12 of 28
    sensors on prod were lost so. The reading is written without the link instead."""
    links = {e["id"].rsplit("-", 1)[1]: e.get("refPointOfInterest", {}).get("object") for e in written["beach-water"]}
    assert links == {"70b3d57050001ab9": BEACH_UNIT.format(41960), "70b3d57050001bbe": BEACH_UNIT.format(40157),
                     "70b3d5705001148e": None}


@requires_docker
def test_a_failed_ask_writes_every_reading_without_its_link():
    """The committed branch itself, its gateway refusing the connection."""
    resources = yaml.safe_load(open_data.RESOURCES.read_text())
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": {"processors": beach_steps()}, "rate_limit_resources": resources["rate_limit_resources"],
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "none"}}
    gone = "http://127.0.0.1:9"
    env = {"JC_ORG_DOMAIN": "hel.fi", "JC_SPACE": "helsinki", "JC_GATEWAY_URL": gone, "JC_CLIENT_ID": "beach",
           "JC_CLIENT_SECRET": "unused", "JC_TOKEN_URL": f"{gone}/token", "CONFIG": json.dumps(config)}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", *(a for k in env for a in ("-e", k)), "--entrypoint", "sh", open_data.BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=json.dumps(fixture("uiras-beaches.json")).encode(), env={**os.environ, **env}, capture_output=True, timeout=120, check=False)
    assert result.returncode == 0, result.stderr.decode()
    (out,) = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]
    assert len(out) == 3 and not any("refPointOfInterest" in e for e in out)
    assert sum("temperature" in e for e in out) == 2


@requires_docker
def test_the_ask_names_each_linked_unit_once_and_a_feed_without_units_asks_nothing():
    steps = beach_steps()
    branch = next(p for p in steps if "branch" in p)
    ask = [*steps[:steps.index(branch)], {"mapping": branch["branch"]["request_map"]}]

    def asked(document: dict) -> list[dict]:
        with tempfile.NamedTemporaryFile("w", suffix=".yaml") as mapping:
            yaml.safe_dump({"pipeline": {"processors": ask}}, mapping)
            mapping.flush()
            return open_data.run_all(Path(mapping.name), json.dumps(document).encode(), "helsinki")

    document = fixture("uiras-beaches.json")
    twice = copy.deepcopy(document)
    twice["features"].append({**copy.deepcopy(document["features"][0]), "id": "70B3D57050001AAA"})
    (query,) = asked(twice)
    assert query["type"] == "Query"
    assert sorted(query["entities"], key=lambda e: e["id"]) == [
        {"id": BEACH_UNIT.format(u), "type": "PointOfInterest"} for u in (40157, 40356, 41960)]
    for feature in document["features"]:
        feature["properties"]["servicemap_url"] = ""
    assert asked(document) == []


def public_water_quality() -> dict:
    return yaml.safe_load((HELSINKI / "helsinki-policy-public-water-quality.yaml").read_text())["spec"]


def test_the_public_beach_water_omits_the_link_to_a_unit_no_public_endpoint_holds():
    """T-5089: 12 of 18 sensors name a Service Map unit the services pipeline never writes."""
    (rule,) = public_water_quality()["information"]
    assert "refPointOfInterest" not in rule["propertyNames"]
    assert "relationshipNames" not in rule


@requires_docker
def test_the_public_beach_water_grants_every_other_attribute_the_pipeline_writes(written):
    (rule,) = public_water_quality()["information"]
    attrs = {a for e in written["beach-water"] for a in e} - {"id", "type", "refPointOfInterest"}
    assert attrs == set(rule["propertyNames"])


@requires_docker
def test_a_parking_area_the_city_has_not_estimated_has_no_capacity_rather_than_zero(written):
    assert sorted("totalSpotNumber" in e for e in written["parking-areas"]) == [False, True, True]
    # The guide names no area; its id's first block does (T-4745).
    for e in written["parking-areas"]:
        block = e["id"].rsplit("parkkiopas-", 1)[1][:8]
        assert e["name"]["languageMap"] == {"fi": f"Pysäköintialue {block}", "en": f"Parking area {block}"}


@requires_docker
def test_districts_read_as_a_person_writes_them_and_sub_districts_keep_the_statistics_code(written):
    district = written["districts"][0]
    assert district["name"]["languageMap"] == {"fi": "Villinki", "sv": "Villinge"}
    assert district["id"].endswith(":district-50")
    codes = {e["districtCode"]["value"] for e in written["subdistricts"]}
    assert all(re.fullmatch(r"091\d{7}", code) for code in codes)
    assert all(e["id"].endswith("subdistrict-" + e["districtCode"]["value"]) for e in written["subdistricts"])


@requires_docker
def test_voting_districts_keep_the_code_the_voting_card_prints(written):
    """T-5256: 052A is voting district A of district 52."""
    by_code = {e["districtCode"]["value"]: e for e in written["voting-districts"]}
    assert set(by_code) == {"052A", "031F", "014C"}
    area = by_code["052A"]
    assert area["id"] == "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:voting-district-052A"
    assert area["divisionLevel"]["value"] == "votingDistrict"
    # The layer leaves the Swedish name empty: no empty text is written.
    assert area["name"]["languageMap"] == {"fi": "Suomenlinna"}


@requires_docker
def test_a_quarter_is_named_by_its_plan_number_unless_the_register_names_it(written):
    names = {e["districtCode"]["value"]: e["name"]["languageMap"] for e in written["quarters"]}
    assert names["0910470363"] == {"fi": "Kortteli 47363", "sv": "Kvarter 47363"}
    assert names["0910100671"]["fi"] != "Kortteli 10671" and names["0910100671"]["sv"]
    for e in written["quarters"]:
        assert e["divisionLevel"]["value"] == "quarter"
        assert e["id"].endswith(":quarter-" + e["districtCode"]["value"])
        assert e["location"]["value"]["type"] == "MultiPolygon"


@requires_docker
@pytest.mark.parametrize("feed, code", [("voting-districts", "tunnus"), ("quarters", "korttelitunnus")])
def test_an_area_without_an_outline_or_with_an_unknown_code_is_dropped_and_a_repeat_is_one_area(feed, code):
    """T-5256: a record with no geometry, one whose code is not the layer's code shape, one of another
    municipality and one listed twice: the first three write nothing, the repeat writes one id."""
    recording = fixture(f"{feed}.json")
    first, second, third = recording["features"]
    second["geometry"] = None
    third["properties"][code] = "X" + str(third["properties"][code])
    # Espoo (049): a quarter code of another municipality, or a voting district the layer marks as Espoo's.
    other = copy.deepcopy(first)
    other["properties"].update({"kunta": "049", code: "0490010001" if feed == "quarters" else "001A"})
    recording["features"].extend([copy.deepcopy(first), other])
    ids = [e["id"] for e in run(feed, recording)]
    assert len(set(ids)) == 1 and ids[0].endswith(str(first["properties"][code]))


@requires_docker
@pytest.mark.parametrize("feed, recording", [("business-plots", "business-plots.json"),
                                             ("environmental-permit-sites", "environmental-permits.json"),
                                             ("preschool-areas", "preschool-areas.json")])
def test_a_record_without_a_point_or_with_an_unknown_id_is_dropped_and_a_repeat_is_one_entity(feed, recording):
    """T-5822: a record with no geometry, one whose id is not the layer's numeric id, and one listed
    twice: the first two write nothing, the repeat writes one id."""
    layer = fixture(recording)
    first, second, third = layer["features"]
    second["geometry"] = None
    third["properties"]["id"] = "X" + str(third["properties"]["id"])
    layer["features"].append(copy.deepcopy(first))
    ids = [e["id"] for e in run(feed, layer)]
    assert len(set(ids)) == 1 and ids[0].endswith("-" + str(first["properties"]["id"]))


@requires_docker
def test_a_zone_link_without_a_scheme_is_written_as_https(written):
    fee = written["fee-parking-zones"][0]
    assert fee["category"]["value"] == ["feeCharged"] and fee["zoneCode"]["value"] == "1"
    assert fee["url"]["value"] == "https://www.hel.fi/pysakointi"
    assert "4 euroa tunnilta" in fee["description"]["languageMap"]["fi"]



@requires_docker
@pytest.mark.parametrize("pipeline, recording", [("fee-parking-zones", "payment-zones.json"),
                                                 ("resident-parking-zones", "resident-zones.json")])
def test_a_zone_link_that_is_no_single_address_writes_no_url(pipeline, recording):
    """Two links or a space in `linkki` is no IRI, and the broker refuses the whole zone over it
    (F-BNT3-018); the zone stays, without its url."""
    document = fixture(recording)
    document["features"] = document["features"][:1]
    document["features"][0]["properties"]["linkki"] = "www.hel.fi/a www.hel.fi/b"
    (zone,) = run(pipeline, document)
    assert "url" not in zone and zone["zoneCode"]["value"]

@requires_docker
def test_a_line_lists_its_stops_in_order_whatever_order_its_rows_come_in(written):
    recording = fixture("hsl-routes.json")
    rows = [f["properties"] for f in recording["features"] if f["properties"]["route_vari"] == "31M1_1"]
    expected = [r["stop_id"] for r in sorted(rows, key=lambda r: r["stop_seque"])]
    recording["features"].reverse()
    for entities in (written["transit-routes"], run("transit-routes", recording)):
        line = next(e for e in entities if e["id"] == "urn:ngsi-ld:PublicTransportRoute:hel.fi:helsinki:31M1-1")
        assert line["stopSequence"]["value"] == expected
        assert line["transportMode"]["value"] == "metro"
        assert line["transportationType"]["value"] == 1
        assert line["location"]["value"]["type"] == "LineString"
        assert len(line["location"]["value"]["coordinates"]) == len(expected)
    alone = next(e for e in written["transit-routes"] if e["id"].endswith(":4625-10"))
    assert alone["location"]["value"]["type"] == "Point", "one stop is no line"
    assert len(alone["stopSequence"]["value"]) == 1


@requires_docker
def test_an_empty_answer_writes_nothing():
    for name, (recording, _, _) in FEEDS.items():
        empty = copy.deepcopy(fixture(recording))
        empty["results" if "results" in empty else "features"] = []
        assert (beach(empty, []) if name == "beach-water" else run(name, empty)) == [], name


def test_a_value_outside_the_model_is_refused_by_the_schema():
    permit = {
        "id": "urn:ngsi-ld:CityWork:hel.fi:helsinki:KP2602482",
        "type": "CityWork",
        "permitKind": {"type": "Property", "value": "excavation"},
        "permitNumber": {"type": "Property", "value": "KP2602482"},
        "permitStatus": {"type": "Property", "value": "cancelled"},
        "permitStart": {"type": "Property", "value": "2026-09-14"},
        "permitEnd": {"type": "Property", "value": "2026-09-17"},
    }
    assert any("cancelled" in message for message in schema_errors(permit))
    permit["permitStatus"]["value"] = "ongoing"
    assert not schema_errors(permit)
    permit["applicant"] = {"type": "Property", "value": "somebody"}
    assert any("applicant" in message for message in schema_errors(permit))


def test_every_feed_is_seeded_as_a_datasource_a_pipeline_and_a_mapping():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())
    public = yaml.safe_load((HELSINKI / "helsinki-policy-public-all.yaml").read_text())
    readable = {e["type"] for rule in public["spec"]["information"] for e in rule["entities"]}
    for name, (_, kind, _) in FEEDS.items():
        pipeline = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())
        datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"]
        assert pipeline["spec"]["output"]["type"] == kind
        # The domain is the organization's, rendered when the repository is loaded (CC-74).
        assert pipeline["spec"]["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:helsinki:helsinki-all"
        # Every pipeline states what it may use on the one node (Development/12 §4).
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 256
        assert datasource["spec"]["http"]["url"].startswith("https://")
        assert kind in model["spec"]["classes"]
        assert kind in readable, f"{kind} is not readable through the public endpoint"
        for seeded in (f"helsinki-datasource-{name}.yaml", f"helsinki-pipeline-{name}.yaml",
                       f"helsinki-pipeline-{name}-bento.yaml"):
            assert seeded in index, seeded
        assert index[f"helsinki-pipeline-{name}-bento.yaml"] == f"projects/helsinki/pipelines/{name}/bento.yaml"


def test_every_wfs_feed_asks_for_wgs84():
    for name in FEEDS:
        url = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())["spec"]["http"]["url"]
        if "kartta.hel.fi" in url:
            # The layers' own system is EPSG:3879; a GeoProperty is WGS84 (CIM 009).
            assert "srsName=EPSG:4326" in url, name


def test_the_model_classes_and_the_manifest_agree():
    linkml = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    manifest = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())
    # The entity types; a value class such as PostalAddress (T-3949) is no type of the space.
    entities = {name for name, body in linkml["classes"].items() if (body or {}).get("is_a") == "Entity"}
    assert set(manifest["spec"]["classes"]) == entities


@requires_docker
def test_the_street_parking_layers_are_the_smart_data_model_on_street_parking_told_apart_by_category(written):
    """T-4329: parking areas, fee zones and resident zones are one SDM type; `category` says which."""
    for name, category in (("parking-areas", "public"), ("fee-parking-zones", "feeCharged"), ("resident-parking-zones", "forResidents")):
        for entity in written[name]:
            assert entity["category"]["value"] == [category], name
            assert open_data.sdm_errors("dataModel.Parking/OnStreetParking", entity) == [], (name, entity["id"])
    # A zone code the model types as an integer would not be the sign's letter: it stays text.
    assert all(isinstance(e["zoneCode"]["value"], str) for e in written["resident-parking-zones"])


@requires_docker
def test_a_line_variant_is_the_smart_data_model_public_transport_route_with_its_gtfs_route_type(written):
    """T-4329: the SDM requires the GTFS route type as a number; the ferry is 4, the metro 1."""
    types = {e["transportMode"]["value"]: e["transportationType"]["value"] for e in written["transit-routes"]}
    assert types["metro"] == 1 and types.get("ferry", 4) == 4 and all(v in range(8) for v in types.values())
    for entity in written["transit-routes"]:
        assert open_data.sdm_errors("dataModel.UrbanMobility/PublicTransportRoute", entity) == [], entity["id"]


@requires_docker
def test_every_district_and_sub_district_is_the_administrative_area_extension(written):
    """T-4338: no official model holds a city district; it is the extension AdministrativeArea
    (PointOfInterest base, which requires `name`)."""
    for name in ("districts", "subdistricts", "voting-districts", "quarters", "preschool-areas"):
        for entity in written[name]:
            assert open_data.sdm_errors("extensions/AdministrativeArea", entity) == [], (name, entity["id"])


@requires_docker
def test_a_business_plot_keeps_its_numbers_in_square_metres_and_a_worded_value_as_text(written):
    """T-5822: pinta_ala and rakennusoikeus are text in the layer; a number becomes plotArea and
    floorAreaRight (MTK, square metres), "noin 640" stays in the description, never guessed."""
    plots = {e["id"].rsplit("-", 1)[1]: e for e in written["business-plots"]}
    office = plots["17812"]
    assert office["serviceCategory"]["value"] == "businessPlot"
    assert office["plotArea"] == {"type": "Property", "value": 3414, "unitCode": "MTK"}
    assert office["floorAreaRight"]["value"] == 10500
    assert office["address"]["value"] == {"streetAddress": "Mannerheimintie 111", "addressLocality": "Helsinki", "addressCountry": "FI"}
    assert office["name"]["languageMap"]["fi"] == "Yritystontti, Mannerheimintie 111"
    lines = office["description"]["languageMap"]["fi"].split("\n")
    assert "Käyttötarkoitus: Toimistorakennusten korttelialue (KT)" in lines and "Kiinteistötunnus: 091-016-0748-0004" in lines
    # A plot only ships may use: the plan gives it no floor area, which is 0 and not absent.
    assert plots["17883"]["floorAreaRight"]["value"] == 0
    worded = plots["17897"]
    assert "plotArea" not in worded and worded["floorAreaRight"]["value"] == 625
    assert "Pinta-ala: noin 640" in worded["description"]["languageMap"]["fi"]
    for entity in written["business-plots"] + written["environmental-permit-sites"]:
        assert open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity) == [], entity["id"]


@requires_docker
def test_an_environmental_permit_site_splits_the_post_office_field_and_names_its_activity(written):
    sites = {e["id"].rsplit("-", 1)[1]: e for e in written["environmental-permit-sites"]}
    station = sites["4961781"]
    assert station["serviceCategory"]["value"] == "environmentalPermitSite"
    assert station["name"]["languageMap"] == {"fi": "Neste Vanha Porvoontie"}
    assert station["address"]["value"] == {"streetAddress": "Vanha Porvoontie 32", "postalCode": "00760", "addressLocality": "Helsinki", "addressCountry": "FI"}
    assert station["description"]["languageMap"]["fi"] == "Toiminta: YSL 527/2014 liite 2.3 jakeluasema"
    # 00099 HELSINGIN KAUPUNKI is the city's own postal code: the town is still Helsinki, the street text trimmed.
    assert sites["5350900"]["address"]["value"]["addressLocality"] == "Helsinki"
    assert sites["5350900"]["address"]["value"]["streetAddress"] == "Sompasaari"
    # A site the register gives no address keeps its point and has no address, rather than an empty one.
    assert "address" not in sites["5430652"] and sites["5430652"]["location"]["value"]["type"] == "Point"


@requires_docker
def test_a_preschool_area_carries_its_school_year_language_and_only_the_units_the_space_holds(written):
    """T-5822: the layer names Service Map units; unit 148 is in the space (the test's ask answers
    so), unit 29859 is not, and a link to it would make the gateway refuse the area (DM-70)."""
    areas = {e["id"].rsplit("-", 1)[1]: e for e in written["preschool-areas"]}
    coming = areas["117314"]
    assert coming["divisionLevel"]["value"] == "preschoolAdmissionArea"
    assert coming["name"]["languageMap"] == {"fi": "Suomenlinna", "sv": "Sveaborg"}
    assert coming["validFrom"]["value"] == "2026-08-01T00:00:00+03:00"
    assert coming["validTo"]["value"] == "2027-07-31T23:59:59+03:00"
    assert coming["description"]["languageMap"]["fi"] == "Suomenkielinen esiopetusalue, lukuvuosi 2026–2027"
    assert coming["serviceUnits"] == {"type": "Relationship", "object": [PRESCHOOL_UNIT.format(148)]}
    assert areas["117214"]["validFrom"]["value"] == "2025-08-01T00:00:00+03:00"
    swedish = areas["116782"]
    assert swedish["name"]["languageMap"] == {"sv": "Drumsö förskola"}
    assert swedish["description"]["languageMap"]["en"] == "Swedish-language preschool admission area, school year 2026–2027"
    assert "serviceUnits" not in swedish
    assert swedish["source"]["value"].endswith("avoindata:Esiopetusalue_ruotsi")
    # A failed ask (no unit held) writes every area without its links.
    assert all("serviceUnits" not in e for e in preschool(fixture("preschool-areas.json"), []))


@requires_docker
def test_every_public_area_permit_is_the_smart_data_model_city_work(written):
    """T-4329: excavations, traffic arrangements and event permits are SDM CityWork."""
    for name in ("excavation", "traffic-arrangements", "area-events"):
        for entity in written[name]:
            assert open_data.sdm_errors("dataModel.Transportation/CityWork", entity) == [], (name, entity["id"])


RSS = """<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"><channel><title>City of Helsinki news</title>
<item><title>Tram 9 runs again</title><link>https://www.hel.fi/en/news/tram-9</link>
<guid isPermaLink="false">hel-news-1001</guid><description>Works on Mannerheimintie end.</description>
<pubDate>Thu, 15 Oct 2026 09:30:00 +0300</pubDate><enclosure url="https://www.hel.fi/img/tram.jpg" type="image/jpeg"/></item>
<item><title>Library hours</title><link>https://www.hel.fi/en/news/library</link><guid>hel-news-1002</guid>
<pubDate>Fri, 16 Oct 2026 08:00:00 +0300</pubDate></item>
<item><title>No guid, no entity</title><link>https://www.hel.fi/en/news/none</link></item>
</channel></rss>"""


@requires_docker
def test_a_news_item_is_the_article_extension_with_its_guid_as_identifier():
    """T-4338: the feed writes the Smart Data Model extension Article (CPSV-AP Evidence base), whose
    base requires `identifier` and `name`; an item without a guid has no identity and is dropped."""
    articles = open_data.run_all(HELSINKI / "helsinki-pipeline-news-bento.yaml", RSS.encode(), "helsinki")
    assert [a["identifier"]["value"] for a in articles] == ["hel-news-1001", "hel-news-1002"]
    for article in articles:
        assert article["type"] == "Article" and article["id"].startswith("urn:ngsi-ld:Article:hel.fi:helsinki:")
        assert schema_errors(article) == [], article["id"]
        assert open_data.sdm_errors("extensions/Article", article) == [], article["id"]
    assert articles[0]["datePublished"]["value"] == "2026-10-15T06:30:00Z"
    # Without its identifier the extension refuses the item.
    bare = {k: v for k, v in articles[0].items() if k != "identifier"}
    assert any("identifier" in e for e in open_data.sdm_errors("extensions/Article", bare))


def test_the_services_mapping_reads_only_the_unit_fields_its_requests_ask_for():
    """F-R2BNT3-005: `address_city` is no Service Map field and in neither `only=` list, so every
    address lost its locality. Each list (the DataSource's and the page request's) must name every
    `unit.<field>` the mapping reads, and both filter to Helsinki, which the locality states."""
    mapping = (HELSINKI / "helsinki-pipeline-services-bento.yaml").read_text()
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-services.yaml").read_text())["spec"]["http"]["url"]
    read = set(re.findall(r"\bunit\.([a-z_]+)", mapping))
    urls = [source, *re.findall(r"url: (https://api\.hel\.fi/servicemap/v2/unit/\S+)", mapping)]
    assert len(urls) == 2 and read
    for url in urls:
        assert "municipality=helsinki" in url, url
        only = set(re.search(r"[?&]only=([^&]+)", url).group(1).split(","))
        assert read <= only, (url, sorted(read - only))
