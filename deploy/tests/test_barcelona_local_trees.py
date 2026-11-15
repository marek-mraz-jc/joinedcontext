"""Barcelona open data, environment rest (T-5413, PL-03, DM-01, DM-61): the trees of local interest.

Bento runs the committed mapping over `fixtures/barcelona/arbres-interes-local.json`: the first tree
of each district (and the one with no district) of the DataStore answer the bcn-local-trees
DataSource's query gave at 2026-10-24T17:20Z (208 trees). Every entity is checked against the
space's generated JSON Schema, which the gateway enforces, and against the Smart Data Model at its
vendored pin. The edge cases are that recording edited in the one place each names. Without a
container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
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
MAPPING = CITY / "barcelona-pipeline-local-trees-bento.yaml"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/barcelona/arbres-interes-local.json").read_text())
PREFIX = "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:local-tree-"
AS_OF = {"bcn_now": "2026-10-24T17:20:00Z"}
BBOX = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]


def run(document: dict) -> list[dict]:
    return open_data.run_all(MAPPING, json.dumps(document).encode(), "barcelona", "barcelona.cat", AS_OF)


def edited(edit) -> str:
    answer = copy.deepcopy(RECORDED)
    edit(answer["result"]["records"])
    return json.dumps(answer)


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in run(RECORDED)}


@requires_docker
def test_each_tree_of_the_register_is_one_point_of_interest(written):
    assert len(written) == 11
    tipuanes = written[PREFIX + "99400454937"]
    assert tipuanes["name"] == {"type": "Property", "value": "Tipuanes de la plaça Sant Felip Neri"}
    assert tipuanes["category"]["value"] == ["environment", "local-interest-tree"]
    assert tipuanes["location"]["value"] == {"type": "Point", "coordinates": [2.175095, 41.383469]}
    assert tipuanes["address"]["value"] == {"streetAddress": "Pl Sant Felip Neri 4", "addressLocality": "Barcelona",
                                            "district": "Ciutat Vella", "addressCountry": "ES"}
    assert tipuanes["expiresAt"] == "2026-11-14T17:20:00Z"
    # A tree without a street number or district keeps the street and leaves the district out.
    assert written[PREFIX + "99400679820"]["address"]["value"] == {
        "streetAddress": "Parc de l'Estació del Nord", "addressLocality": "Barcelona", "addressCountry": "ES"}


@requires_docker
def test_every_tree_is_one_the_gateway_accepts_valid_in_its_smart_data_model_and_inside_the_city(written):
    for entity in written.values():
        assert open_data.schema_errors(CITY / "barcelona.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]


@requires_docker
def test_an_out_of_area_point_a_nameless_tree_and_a_duplicate_are_dropped_and_counted():
    def edit(rows):
        rows[0]["lon"] = "2.9"  # Girona's longitude: outside Barcelona's 1 km margin
        rows[1]["name"] = " "
        rows[2]["lat"] = "n/a"
        rows.append(dict(rows[3]))  # the same id twice
        rows.append(dict(rows[4], id="99400999999"))  # a second id on the same point
    answer = first_step(edited(edit), AS_OF, MAPPING)
    assert answer["dropped"] == {"noIdOrName": 1, "noPoint": 1, "outsideTheCity": 1, "duplicateId": 1,
                                 "samePoint": 1}
    assert len(answer["trees"]) == 13 - 5


@requires_docker
def test_a_refused_query_an_empty_answer_and_an_oversized_one_fail_the_run():
    refused = {"success": False, "error": {"message": "relation does not exist"}}
    assert "refused the trees query" in first_step(json.dumps(refused), AS_OF, MAPPING)["error"]
    assert "holds no tree" in first_step(edited(lambda rows: rows.clear()), AS_OF, MAPPING)["error"]
    big = edited(lambda rows: rows.append({"pad": " " * 2097152}))
    assert "more than the 2 MiB read" in first_step(big, AS_OF, MAPPING)["error"]


def test_the_trees_upsert_from_the_city_and_publish_only_their_own_ids():
    pipeline = yaml.safe_load((CITY / "barcelona-pipeline-local-trees.yaml").read_text())["spec"]
    source = yaml.safe_load((CITY / "barcelona-datasource-local-trees.yaml").read_text())["spec"]["http"]
    endpoint = yaml.safe_load((CITY / "barcelona-endpoint-local-trees.yaml").read_text())["spec"]
    policy = yaml.safe_load((CITY / "barcelona-policy-public-local-trees.yaml").read_text())["spec"]
    assert source["url"].startswith("https://opendata-ajuntament.barcelona.cat/")
    # A read-only query with its own row cap.
    assert "SELECT" in source["url"] and "LIMIT%201000" in source["url"]
    assert pipeline["source"]["dataSourceRef"]["name"] == "bcn-local-trees"
    assert pipeline["output"] == {"type": "PointOfInterest", "mode": "upsert"}
    assert endpoint["policyRef"].endswith(":public-local-trees")
    assert endpoint["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "local-trees"}
    assert endpoint["publish"]["ckan"]["instanceRef"]["name"] == "barcelona"
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert not {"upsertBatch", "createBatch", "deleteBatch", "createEntity", "deleteEntity"} & set(policy["operations"])
    [grant] = policy["information"][0]["entities"]
    assert grant["type"] == "PointOfInterest"
    assert re.match(grant["idPattern"], PREFIX + "99400454937")
    assert not re.match(grant["idPattern"], "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:99400454937")
    assert not re.match(grant["idPattern"], "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:dog-area-977625")
