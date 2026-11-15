"""Barcelona's licensed tourist flats as SDM TouristRental at building level (T-5417, PL-03, DM-01).

Bento runs the committed mapping over `fixtures/barcelona/habitatges-us-turistic.csv`: 9 rows of
the DataStore resource hut_comunicacio_opendata.csv as the DataSource's URL answered it on
2026-10-23 (15:54Z), one register number with trailing no-break spaces, one with a blank inside
(`HUTB- 078453`), one that is no register number, one row without places or register, a
building with a number range and a letter. The household columns (BLOC, PORTAL, ESCALA, PIS,
PORTA) are replaced by markers ending in 7Q, so a test can show none of them is ever written.
Every entity is checked against the space's generated JSON Schema, which the gateway enforces,
and against SDM TouristRental at the vendored pin. The edge cases are the recording edited in
the one place each names. Without a container runtime the Bento cases skip; the seed cases do not.
"""

import csv
import io
import json
import os
import tempfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-tourism")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = CITY / "barcelona-pipeline-tourist-flats-bento.yaml"
FIXTURE = Path(__file__).resolve().parent / "fixtures/barcelona/habitatges-us-turistic.csv"
FLAT = "urn:ngsi-ld:TouristRental:barcelona.cat:barcelona:"
AS_OF = {"bcn_now": "2026-10-23T06:00:00Z"}
BBOX = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]
HOUSEHOLD = ("BLOC", "PORTAL", "ESCALA", "PIS", "PORTA")


def run(document: str, now: dict | None = AS_OF) -> list[dict]:
    return open_data.run_all(MAPPING, document.encode(), "barcelona", "barcelona.cat", now)


def first_step(document: str, now: dict | None = AS_OF) -> dict:
    """The first step's `dropped` counts and flats, or the error it raised. The trimmed pipeline
    is a temp file of its own, so xdist workers never share it and no seed glob reads it."""
    handle, name = tempfile.mkstemp(suffix=".yaml")
    os.close(handle)
    trimmed = Path(name)
    pipeline = yaml.safe_load(MAPPING.read_text())
    pipeline["pipeline"]["processors"] = pipeline["pipeline"]["processors"][:1] + [
        {"mapping": 'root = if errored() { { "error": error() } } else { this }'}]
    trimmed.write_text(yaml.safe_dump(pipeline))
    try:
        return open_data.run_all(trimmed, document.encode(), "barcelona", "barcelona.cat", now)[0]
    finally:
        trimmed.unlink()


def edited(edit) -> str:
    rows = list(csv.reader(io.StringIO(FIXTURE.read_text())))
    edit(rows)
    out = io.StringIO()
    csv.writer(out, lineterminator="\n").writerows(rows)
    return out.getvalue()


@pytest.fixture(scope="module")
def flats():
    return {e["id"]: e for e in run(FIXTURE.read_text())}


@requires_docker
def test_every_licence_is_one_flat_at_its_building_with_places_and_register(flats):
    rows = list(csv.DictReader(io.StringIO(FIXTURE.read_text())))
    assert sorted(flats) == sorted(FLAT + r["N_EXPEDIENT"] for r in rows)
    raval = flats[FLAT + "01-2009-0354"]
    assert raval["name"]["value"] == "Habitatge d'ús turístic HUTB-002222"
    assert raval["capacity"] == {"type": "Property", "value": 3}
    assert raval["registerNumber"]["value"] == "HUTB-002222"
    assert raval["address"]["value"] == {"streetAddress": "Carrer SANT JOSEP ORIOL, 19", "district": "CIUTAT VELLA",
                                         "addressLocality": "Barcelona", "addressCountry": "ES"}
    assert raval["areaServed"]["value"] == "el Raval"
    assert raval["location"]["value"] == {"type": "Point", "coordinates": [2.170172, 41.378445]}
    assert raval["expiresAt"] == "2026-11-13T06:00:00Z"
    # A number range with a letter; register numbers with blanks read as the number they are.
    assert flats[FLAT + "05-2010-0075"]["address"]["value"]["streetAddress"] == "Carrer ESCIPIO, 36B-38"
    assert flats[FLAT + "01-2016-0478"]["registerNumber"]["value"] == "HUTB-016458"
    assert flats[FLAT + "01-2025-0033"]["registerNumber"]["value"] == "HUTB-078453"
    # No register number and no places: the licence stands, named by its file, without either.
    bare = flats[FLAT + "01-2019-0368"]
    assert "registerNumber" not in bare and "capacity" not in bare
    assert bare["name"]["value"] == "Habitatge d'ús turístic 01-2019-0368"


@requires_docker
def test_no_flat_carries_its_floor_door_staircase_or_block(flats):
    text = json.dumps(list(flats.values()))
    assert "7Q" not in text, "a household field reached an entity"
    mapping = MAPPING.read_text().split("\npipeline:", 1)[1]
    assert not [c for c in HOUSEHOLD if f"r.{c}" in mapping or f'"{c}"' in mapping], "the mapping reads a household column"


@requires_docker
def test_every_flat_is_one_the_gateway_and_the_smart_data_model_accept_inside_the_city(flats):
    assert flats
    for entity in flats.values():
        assert open_data.schema_errors(CITY / "barcelona.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.TourismDestinations/TouristRental", entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]


@requires_docker
def test_an_outside_point_a_negative_capacity_and_duplicates_are_each_dropped_and_counted():
    def edit(rows):
        rows.append(list(rows[1]))                          # 01-2009-0354 twice
        rows.append(list(rows[2]))                          # HUTB-016458 under a second licence file
        rows[-1][0] = "01-2016-9999"
        rows[3][19], rows[3][20] = rows[3][20], rows[3][19]  # 01-2025-0033 with longitude and latitude swapped
        rows[7][18] = "-4"                                  # 10-2026-0025 with -4 places
        rows[8][19] = "abc"                                 # 06-2008-0125 with no number for a point
        rows[9][0] = "06/2008/0138"                         # a licence file the register never writes
    baseline = first_step(FIXTURE.read_text())["dropped"]
    assert baseline == {"noLicenceId": 0, "noPoint": 0, "outsideTheCity": 0, "duplicateLicence": 0,
                        "duplicateRegisterNumber": 0, "placesMissing": 2, "placesOutOfRange": 0,
                        "registerNumberMalformed": 1}
    answer = first_step(edited(edit))
    assert answer["dropped"] == {**baseline, "noLicenceId": 1, "noPoint": 1, "outsideTheCity": 1, "duplicateLicence": 1,
                                 "duplicateRegisterNumber": 1, "placesOutOfRange": 1}
    ids = [f["id"] for f in answer["flats"]]
    assert ids.count("01-2009-0354") == 1 and "01-2016-9999" not in ids
    assert not {"01-2025-0033", "06-2008-0125", "06/2008/0138"} & set(ids)
    entities = {e["id"]: e for e in run(edited(edit))}
    assert "capacity" not in entities[FLAT + "10-2026-0025"], "a negative place count is left out, the flat stays"


@requires_docker
def test_an_empty_a_stale_or_a_reshaped_file_fails_the_run_instead_of_writing():
    lines = FIXTURE.read_text().splitlines()
    assert "holds no licence" in first_step(lines[0] + "\n")["error"]
    # The newest licence file in the recording is from 2026: two years on, the register is stale.
    assert "is stale: its newest licence file is from 2026" in first_step(FIXTURE.read_text(), {"bcn_now": "2028-01-02T00:00:00Z"})["error"]
    assert "lacks a column" in first_step(FIXTURE.read_text().replace("NUMERO_PLACES", "PLACES", 1))["error"]


def test_the_seed_wires_one_pipeline_one_public_endpoint_and_a_policy_of_its_own_type():
    docs = {}
    for path in CITY.glob("*tourist-flats*.yaml"):
        if not path.name.endswith("-bento.yaml"):
            doc = yaml.safe_load(path.read_text())
            docs[doc["kind"]] = doc["spec"]
    assert docs["Pipeline"]["output"] == {"type": "TouristRental", "mode": "upsert"}, "upsert: an empty file never deletes"
    assert "period" not in docs["Pipeline"] and len(docs["Pipeline"]["schedule"].split()) == 5
    assert docs["DataSource"]["http"]["url"].startswith("https://opendata-ajuntament.barcelona.cat/data/dataset/")
    assert docs["Endpoint"]["policyRef"] == "urn:ngsi-ld:Policy:barcelona.cat:barcelona:public-tourist-flats"
    assert docs["Endpoint"]["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "barcelona"}
    assert docs["Policy"]["assignee"] == {"kind": "role", "id": "public"}
    assert [e["type"] for rule in docs["Policy"]["information"] for e in rule["entities"]] == ["TouristRental"]
    assert all(op.startswith(("retrieve", "createSub", "updateSub", "deleteSub")) for op in docs["Policy"]["operations"])
