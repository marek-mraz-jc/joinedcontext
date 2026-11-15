"""T-5823: ten layers of the city's nature information system (LTJ_avoin_*) from kartta.hel.fi in the
Helsinki space (PL-03, DM-61).

Bento runs the committed mapping over the WFS answers the deployer recorded on 2026-10-26 (T-5668,
`fixtures/helsinki/kartta/`), joined into one answer as GeoServer gives them for typeNames=(a)(b)...,
and every entity it writes is checked against the space's generated JSON Schema (NatureSite has no
Smart Data Model). The edge cases are those answers edited in the one place they name.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import re
import urllib.parse
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-nature")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
KARTTA = Path(__file__).resolve().parent / "fixtures/helsinki/kartta"
WFS = ("https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&version=2.0.0&request=GetFeature"
       "&outputFormat=application/json&srsName=EPSG:4326&typeNames=")
LAYERS = {
    "LTJ_avoin_rauhoitettu_luonnonsuojelualueet": "natureReserve",
    "LTJ_avoin_rauhoitettu_Natura_aluemaiset": "natura2000",
    "LTJ_avoin_rauhoitettu_luonnonmuistomerkit": "naturalMonument",
    "LTJ_avoin_rauhoitettu_suojellut_luontotyypit": "protectedHabitat",
    "LTJ_avoin_rauhoitettavat_luonnonsuojeluohjelma": "conservationProgramme",
    "LTJ_avoin_arvo_liito_orava_ydinalueet": "flyingSquirrelCore",
    "LTJ_avoin_arvo_tarkeat_lintualueet": "birdArea",
    "LTJ_avoin_arvo_tarkeat_lepakkoalueet": "batArea",
    "LTJ_avoin_arvokkaat_kasvikohteet": "valuablePlantSite",
    "LTJ_avoin_arvokkaat_geologiset_aluemaiset": "geologicalSite",
}
# Turns a failed run into one message naming its error, as the runner's log reads it.
FAILED = ({"catch": [{"mapping": 'root = {"failed": error()}'}]},)


def recording(layer: str) -> dict:
    return json.loads((KARTTA / f"{layer}.json").read_text())


def answer() -> dict:
    layers = [recording(layer) for layer in LAYERS]
    return {**layers[0], "features": [f for layer in layers for f in layer["features"]]}


def run(document: dict) -> list[dict]:
    return open_data.run_all(HELSINKI / "helsinki-pipeline-nature-bento.yaml", json.dumps(document).encode(),
                             "helsinki", then=FAILED)


def site_id(feature: dict) -> str:
    return f"urn:ngsi-ld:NatureSite:hel.fi:helsinki:nature-{feature['properties']['id']}"


@pytest.fixture(scope="module")
def written():
    return run(answer())


@requires_docker
def test_each_recorded_site_is_written_once_with_its_layer_and_the_gateway_accepts_it(written):
    features = answer()["features"]
    assert len(written) == len(features) == len({e["id"] for e in written})
    assert {e["id"] for e in written} == {site_id(f) for f in features}
    assert {e["natureSiteCategory"]["value"] for e in written} == set(LAYERS.values())
    for entity in written:
        assert entity["type"] == "NatureSite"
        assert re.fullmatch(r"urn:ngsi-ld:NatureSite:hel\.fi:helsinki:nature-[0-9]+", entity["id"]), entity["id"]
        assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], (entity["id"],)
        assert entity["dataProvider"]["value"]
        layer = entity["source"]["value"].removeprefix("https://kartta.hel.fi/ws/geoserver/avoindata/wfs?typeNames=avoindata:")
        assert LAYERS[layer] == entity["natureSiteCategory"]["value"]
        assert entity["expiresAt"]


@requires_docker
def test_a_site_carries_its_name_value_class_area_plain_text_and_outline(written):
    feature = recording("LTJ_avoin_rauhoitettu_luonnonsuojelualueet")["features"][0]
    site = next(e for e in written if e["id"] == site_id(feature))
    assert feature["properties"]["nimi"] == "Solhälla"
    assert site["name"]["languageMap"] == {"fi": "Solhälla"}
    assert site["natureSiteCategory"]["value"] == "natureReserve" and "valueClass" not in site
    assert site["areaHectares"] == {"type": "Property", "value": 1.8, "unitCode": "HAR"}
    text = site["description"]["languageMap"]["fi"]
    assert text.startswith("Alue on pääosin vanhaa kangasmetsää") and "<" not in text and "&" not in text
    assert site["location"]["value"] == feature["geometry"]
    graded = recording("LTJ_avoin_arvokkaat_geologiset_aluemaiset")["features"][0]
    assert next(e for e in written if e["id"] == site_id(graded))["valueClass"]["value"] == graded["properties"]["arvoluokan_selite"]
    monument = recording("LTJ_avoin_rauhoitettu_luonnonmuistomerkit")["features"][0]
    point = next(e for e in written if e["id"] == site_id(monument))
    assert point["location"]["value"]["type"] == "Point" and "areaHectares" not in point, "a point has no area"


@requires_docker
def test_a_site_of_an_unknown_layer_without_an_id_or_outline_or_outside_helsinki_is_dropped():
    document = answer()
    good = document["features"]
    base = good[0]

    def site(key, layer="LTJ_avoin_rauhoitettu_luonnonsuojelualueet", geometry="keep", **properties) -> dict:
        feature = copy.deepcopy(base)
        feature["id"] = f"{layer}.fid-x{key}"
        feature["properties"].update({"id": key, **properties})
        if geometry != "keep":
            feature["geometry"] = geometry
        return feature

    bad = [
        site(990001, layer="LTJ_avoin_muu_elainhavaintoja"),
        site(None),
        site("990002"),
        site(990003, geometry=None),
        site(990004, geometry={"type": "LineString", "coordinates": [[24.9, 60.2], [24.95, 60.2]]}),
        site(990005, geometry={"type": "Point", "coordinates": [13.4, 52.5]}),
        site(990006, geometry={"type": "Point", "coordinates": [None, 60.2]}),
        copy.deepcopy(good[1]),
    ]
    kept = [site(990007, nimi="  ", kohdeteksti=None, pinta_ala_ha=-3, arvoluokan_selite="")]
    out = run({**document, "features": good + bad + kept})
    assert sorted(e["id"] for e in out) == sorted([site_id(f) for f in good] + [site_id(kept[0])])
    odd = next(e for e in out if e["id"] == site_id(kept[0]))
    assert {"name", "description", "areaHectares", "valueClass"}.isdisjoint(odd), "empty or impossible values are left out"
    assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", odd) == []


@requires_docker
def test_an_answer_that_is_mostly_unusable_or_no_feature_list_fails_the_run():
    moved = answer()
    count = len(moved["features"])
    broken = count // 2 + 1
    for feature in moved["features"][:broken]:
        feature["geometry"] = None
    for document, message in ((moved, f"only {count - broken} of {count} sites"),
                              ({"type": "FeatureCollection", "features": []}, "no site"),
                              ({"exceptionCode": "OperationProcessingFailed"}, "no site")):
        out = run(document)
        assert len(out) == 1 and "failed" in out[0], out[:1]
        assert message in out[0]["failed"]


def test_the_source_reads_the_ten_layers_in_one_query_over_https_with_a_timeout_and_no_credentials():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    datasource = yaml.safe_load((HELSINKI / "helsinki-datasource-nature.yaml").read_text())
    assert datasource["metadata"]["name"] == "hel-nature"
    http = datasource["spec"]["http"]
    assert http["url"] == WFS + "".join(f"%28avoindata:{layer}%29" for layer in LAYERS), "one query per layer, not a join"
    assert http["verb"] == "GET" and http["timeout"] == "60s"
    assert "auth" not in json.dumps(http).lower() and "secret" not in json.dumps(http).lower()
    assert urllib.parse.urlsplit(http["url"]).hostname == "kartta.hel.fi"
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-nature.yaml").read_text())["spec"]
    assert pipeline["source"]["dataSourceRef"]["name"] == "hel-nature"
    assert pipeline["output"] == {"type": "NatureSite", "mode": "upsert"} and pipeline["period"] == "24h"
    assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:helsinki:helsinki-all"
    steps = yaml.safe_load((HELSINKI / "helsinki-pipeline-nature-bento.yaml").read_text())["pipeline"]["processors"]
    assert [list(step) for step in steps] == [["try"]]
    assert [list(step) for step in steps[0]["try"]] == [["mapping"], ["log"], ["unarchive"]]
    for file in ("helsinki-datasource-nature.yaml", "helsinki-pipeline-nature.yaml", "helsinki-pipeline-nature-bento.yaml",
                 "helsinki-endpoint-nature.yaml", "helsinki-policy-public-nature.yaml"):
        assert file in index, file


def test_the_mapping_knows_exactly_the_layers_the_source_reads():
    text = (HELSINKI / "helsinki-pipeline-nature-bento.yaml").read_text()
    assert dict(re.findall(r'"(LTJ_avoin_\w+)": "(\w+)"', text)) == LAYERS


def test_the_endpoint_reads_only_its_type_and_the_model_declares_it():
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-nature.yaml").read_text())["spec"]
    assert endpoint["audience"] == "public" and endpoint["policyRef"] == "urn:ngsi-ld:Policy:hel.fi:helsinki:public-nature"
    assert re.fullmatch(r"[a-z2-7]{32}", endpoint["slug"])
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-nature.yaml").read_text())["spec"]
    assert policy["information"] == [{"entities": [{"type": "NatureSite"}]}]
    assert not any(op.startswith(("create", "update", "delete", "append", "merge", "replace", "upsert"))
                   for op in policy["operations"] if "Subscription" not in op)
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())["spec"]
    assert "NatureSite" in model["classes"]
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())["definitions"]
    linkml = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    assert schema["NatureSiteCategory"]["enum"] == list(linkml["enums"]["NatureSiteCategory"]["permissible_values"])
    assert set(schema["NatureSiteCategory"]["enum"]) == set(LAYERS.values())
