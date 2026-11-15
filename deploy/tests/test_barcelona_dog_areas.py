"""Barcelona open data, environment rest (T-5413, PL-03, DM-01, DM-61): the city's dog areas.

Bento runs the committed mapping over `fixtures/barcelona/arees-gossos.csv`: the header and the
first dog area of each of the ten districts of Open Data BCN's arees_gossos_i_esbarjo_gossos.csv as
the DataSource's URL answered it at 2026-10-23T15:55Z (92 areas, last-modified 2026-10-20). Every
entity is checked against the space's generated JSON Schema, which the gateway enforces, and
against the Smart Data Model at its vendored pin. The edge cases are that recording edited in the
one place each names. Without a container runtime the Bento cases skip; the seed cases do not.
"""

import csv
import io
import re
import shutil
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_barcelona_environment import first_step

pytestmark = pytest.mark.xdist_group("docker-barcelona-environment")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = CITY / "barcelona-pipeline-dog-areas-bento.yaml"
RECORDED = (Path(__file__).resolve().parent / "fixtures/barcelona/arees-gossos.csv").read_text()
PREFIX = "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:dog-area-"
AS_OF = {"bcn_now": "2026-10-23T15:55:00Z"}
BBOX = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]


def run(document: str) -> list[dict]:
    return open_data.run_all(MAPPING, document.encode(), "barcelona", "barcelona.cat", AS_OF)


def edited(edit) -> str:
    rows = list(csv.reader(io.StringIO(RECORDED)))
    edit(rows)
    out = io.StringIO()
    csv.writer(out, quoting=csv.QUOTE_ALL, lineterminator="\n").writerows(rows)
    return out.getvalue()


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in run(RECORDED)}


@requires_docker
def test_each_dog_area_of_the_file_is_one_point_of_interest(written):
    assert len(written) == 10
    trajana = written[PREFIX + "977625"]
    assert trajana["name"] == {"type": "Property",
                               "value": "Àrea de gossos, Trajana, Via, c. - Santander, c. - Verneda, pg. - Binéfar, c."}
    assert trajana["category"]["value"] == ["animals-plants", "dog-area"]
    assert trajana["location"]["value"] == {"type": "Point", "coordinates": [2.204508, 41.427625]}
    assert trajana["address"]["value"] == {"streetAddress": "Via Trajana, 27", "addressLocality": "Barcelona",
                                           "district": "Sant Martí", "addressCountry": "ES"}
    assert trajana["expiresAt"] == "2026-11-13T15:55:00Z"


@requires_docker
def test_every_dog_area_is_one_the_gateway_accepts_valid_in_its_smart_data_model_and_inside_the_city(written):
    for entity in written.values():
        assert open_data.schema_errors(CITY / "barcelona.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]


@requires_docker
def test_an_out_of_area_point_an_out_of_range_area_and_a_duplicate_are_dropped_and_counted():
    def edit(rows):
        header = rows[0]
        col = {name: header.index(name) for name in ("id", "latitud", "longitud", "superficie_m2", "tipus_id")}
        rows[1][col["longitud"]] = "2.9"  # Girona's longitude: outside Barcelona's 1 km margin
        rows[2][col["superficie_m2"]] = "0"
        rows[3][col["superficie_m2"]] = "120000"
        rows.append(list(rows[4]))  # the same id twice
        moved = list(rows[5])
        moved[col["id"]] = "999999"
        rows.append(moved)  # a second id on the same point
        rows[6][col["tipus_id"]] = "12"
        rows[7][col["latitud"]] = "n/a"
    answer = first_step(edited(edit), AS_OF, MAPPING)
    assert answer["dropped"] == {"noIdOrKind": 1, "noPoint": 1, "outsideTheCity": 1, "areaOutOfRange": 2,
                                 "duplicateId": 1, "samePoint": 1}
    # Ten recorded areas and two added rows, seven of them dropped.
    assert len(answer["areas"]) == 12 - 7


@requires_docker
def test_a_file_without_a_dog_area_fails_the_run_and_writes_nothing():
    assert "holds no dog area" in first_step(RECORDED.splitlines()[0] + "\n", AS_OF, MAPPING)["error"]
    assert "more than the 2 MiB read" in first_step(RECORDED + " " * 2097152, AS_OF, MAPPING)["error"]


def test_the_dog_areas_upsert_from_the_city_and_publish_only_their_own_ids():
    pipeline = yaml.safe_load((CITY / "barcelona-pipeline-dog-areas.yaml").read_text())["spec"]
    source = yaml.safe_load((CITY / "barcelona-datasource-dog-areas.yaml").read_text())["spec"]["http"]
    endpoint = yaml.safe_load((CITY / "barcelona-endpoint-dog-areas.yaml").read_text())["spec"]
    policy = yaml.safe_load((CITY / "barcelona-policy-public-dog-areas.yaml").read_text())["spec"]
    assert source["url"].startswith("https://opendata-ajuntament.barcelona.cat/")
    assert pipeline["source"]["dataSourceRef"]["name"] == "bcn-dog-areas"
    assert pipeline["output"] == {"type": "PointOfInterest", "mode": "upsert"}
    assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:barcelona.cat:barcelona:barcelona"
    assert endpoint["policyRef"].endswith(":public-dog-areas")
    assert endpoint["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "dog-areas"}
    # Off the catalogue while prod holds none of it (T-5788, owner 2026-10-26): no publish block, harvest false.
    assert "publish" not in endpoint and endpoint["catalog"]["harvest"] is False
    assert {"ngsi-ld", "csv", "ogc-features", "odata"} <= set(endpoint["enabledRepresentations"])
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert not {"upsertBatch", "createBatch", "deleteBatch", "createEntity", "deleteEntity"} & set(policy["operations"])
    [grant] = policy["information"][0]["entities"]
    assert grant["type"] == "PointOfInterest"
    # The dog areas and no facility of the register, whose ids are bare register numbers.
    assert re.match(grant["idPattern"], PREFIX + "977625")
    assert not re.match(grant["idPattern"], "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:99400680094")
