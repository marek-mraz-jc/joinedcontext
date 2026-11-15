"""Barcelona open data, architectural heritage (T-6570, PL-03, DM-01, DM-61): the buildings with an IPAC code.

The city's own catalogue (patrimoni-arquitectonic-protegit) is a zipped Shapefile in ETRS89 UTM 31N
with no address, which a pipeline cannot read, so the bcn-heritage DataSource asks Wikidata (CC0)
for every item with an IPAC code inside the municipality. Bento runs the committed mapping over
`fixtures/barcelona/wikidata-heritage.json`: the ten buildings within 300 m of 41.3815,2.1530
(Carrer del Consell de Cent at Rocafort) and three others (a World Heritage Site, a BCIN, an urban
protection) of the SPARQL answer the DataSource's query gave at 2026-11-03T07:09Z (1,484
buildings). Every entity is checked against the space's generated JSON Schema, which the gateway
enforces, and against the Smart Data Model at its vendored pin. The edge cases are that recording
edited in the one place each names. Without a container runtime the Bento cases skip; the seed
cases do not.
"""

import copy
import json
import re
import shutil
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_barcelona_environment import first_step

pytestmark = pytest.mark.xdist_group("docker-barcelona-environment")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = CITY / "barcelona-pipeline-heritage-bento.yaml"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/barcelona/wikidata-heritage.json").read_text())
PREFIX = "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:heritage-"
AS_OF = {"bcn_now": "2026-11-03T07:09:00Z"}
BBOX = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]


def run(document: dict) -> list[dict]:
    return open_data.run_all(MAPPING, json.dumps(document).encode(), "barcelona", "barcelona.cat", AS_OF)


def edited(edit) -> str:
    answer = copy.deepcopy(RECORDED)
    edit(answer["results"]["bindings"])
    return json.dumps(answer)


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in run(RECORDED)}


@requires_docker
def test_each_building_is_one_heritage_point_of_interest(written):
    assert len(written) == 13
    golferichs = written[PREFIX + "Q11912729"]
    assert golferichs["name"] == {"type": "Property", "value": "Casa Golferichs"}
    assert golferichs["category"]["value"] == ["heritage", "bcil"]
    assert golferichs["location"]["value"] == {"type": "Point", "coordinates": [2.156199, 41.380739]}
    assert golferichs["address"]["value"] == {"streetAddress": "Viladomat, 120 i Gran Via de les Corts Catalanes, 491",
                                              "addressLocality": "Barcelona", "addressCountry": "ES"}
    assert golferichs["source"]["value"] == "https://www.wikidata.org/wiki/Q11912729"
    assert golferichs["expiresAt"] == "2026-11-24T07:09:00Z"
    # Designations in their fixed order, strongest first; a building with no address keeps the town.
    assert written[PREFIX + "Q48435"]["category"]["value"] == ["heritage", "world-heritage", "bic", "bcin"]
    assert written[PREFIX + "Q98756028"]["category"]["value"] == ["heritage", "bpcc", "urban-protection"]
    assert written[PREFIX + "Q116186180"]["address"]["value"] == {"addressLocality": "Barcelona", "addressCountry": "ES"}


@requires_docker
def test_every_building_is_one_the_gateway_accepts_valid_in_its_smart_data_model_and_inside_the_city(written):
    for entity in written.values():
        assert open_data.schema_errors(CITY / "barcelona.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]


@requires_docker
def test_a_name_falls_back_to_spanish_then_english_and_an_unknown_designation_writes_no_code():
    def edit(rows):
        del rows[9]["ca"]
        del rows[10]["ca"], rows[10]["es"]
        rows[2]["heritage"]["value"] = "Q116890002"
    buildings = {b["id"]: b for b in first_step(edited(edit), AS_OF, MAPPING)["buildings"]}
    assert buildings["Q11912729"]["name"] == RECORDED["results"]["bindings"][9]["es"]["value"]
    assert buildings["Q48435"]["name"] == RECORDED["results"]["bindings"][10]["en"]["value"]
    assert buildings["Q116186183"]["codes"] == []


@requires_docker
def test_an_out_of_area_point_a_nameless_building_a_foreign_item_and_a_duplicate_are_dropped_and_counted():
    def edit(rows):
        rows[0]["coord"]["value"] = "Point(2.9 41.38)"  # Girona's longitude: outside Barcelona's 1 km margin
        for lang in ("ca", "es", "en"):
            rows[1].pop(lang, None)
        rows[2]["coord"]["value"] = "Point(n/a)"
        rows[3]["item"]["value"] = "https://example.org/entity/Q1"
        rows.append(copy.deepcopy(rows[4]))  # the same item twice
        rows.append(dict(copy.deepcopy(rows[5]), item={"type": "uri", "value": "http://www.wikidata.org/entity/Q99999999"}))
    answer = first_step(edited(edit), AS_OF, MAPPING)
    assert answer["dropped"] == {"noItemOrName": 2, "noPoint": 1, "outsideTheCity": 1, "duplicateItem": 1}
    # Two items on one point both stay: Wikidata lists paired houses as two items.
    assert len(answer["buildings"]) == 15 - 5


@requires_docker
def test_a_non_sparql_answer_an_empty_one_a_full_limit_and_an_oversized_one_fail_the_run():
    assert "no SPARQL result" in first_step(json.dumps({"error": "timeout"}), AS_OF, MAPPING)["error"]
    assert "holds no building" in first_step(edited(lambda rows: rows.clear()), AS_OF, MAPPING)["error"]
    full = edited(lambda rows: rows.extend([rows[0]] * (5000 - len(rows))))
    assert "its LIMIT 5000" in first_step(full, AS_OF, MAPPING)["error"]
    big = edited(lambda rows: rows.append({"pad": " " * 8388608}))
    assert "more than the 8 MiB read" in first_step(big, AS_OF, MAPPING)["error"]


def test_the_buildings_upsert_from_wikidata_and_publish_only_their_own_ids():
    pipeline = yaml.safe_load((CITY / "barcelona-pipeline-heritage.yaml").read_text())["spec"]
    source = yaml.safe_load((CITY / "barcelona-datasource-heritage.yaml").read_text())["spec"]["http"]
    endpoint = yaml.safe_load((CITY / "barcelona-endpoint-heritage.yaml").read_text())["spec"]
    policy = yaml.safe_load((CITY / "barcelona-policy-public-heritage.yaml").read_text())["spec"]
    index = yaml.safe_load((CITY / "index.yaml").read_text())
    url = urlsplit(source["url"])
    assert (url.scheme, url.netloc, url.path) == ("https", "query.wikidata.org", "/sparql")
    # A read-only SELECT inside Barcelona (Q1492) with its own row cap, which the mapping checks.
    [query] = parse_qs(url.query)["query"]
    assert query.startswith("SELECT ") and "wd:Q1492" in query and query.endswith("LIMIT 5000")
    assert not re.search(r"\b(INSERT|DELETE|LOAD|CLEAR|SERVICE)\b", query)
    assert source["headers"]["User-Agent"].startswith("joinedcontext-platform/"), "Wikidata refuses anonymous agents"
    assert pipeline["source"]["dataSourceRef"]["name"] == "bcn-heritage"
    assert pipeline["output"] == {"type": "PointOfInterest", "mode": "upsert"}
    assert endpoint["policyRef"].endswith(":public-heritage")
    assert endpoint["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "heritage"}
    assert endpoint["catalog"]["license"] == "CC0"
    assert endpoint["publish"]["ckan"]["instanceRef"]["name"] == "barcelona"
    assert re.fullmatch(r"[a-z2-7]{32}", endpoint["slug"])
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert not {"upsertBatch", "createBatch", "deleteBatch", "createEntity", "deleteEntity"} & set(policy["operations"])
    [grant] = policy["information"][0]["entities"]
    assert grant["type"] == "PointOfInterest"
    assert re.match(grant["idPattern"], PREFIX + "Q11912729")
    assert not re.match(grant["idPattern"], "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:99400454937")
    assert not re.match(grant["idPattern"], "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:local-tree-99400454937")
    for name in ("datasource", "pipeline", "pipeline-heritage-bento", "endpoint", "policy-public"):
        assert any(name in key and "heritage" in key for key in index), name
