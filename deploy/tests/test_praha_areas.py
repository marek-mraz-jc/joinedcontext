"""Prague's IPR place layers drawn as areas or lines (T-5426, PL-03): cemeteries, nature trails,
water areas and streams, dog zones (PointOfInterest), the planning balance units
(AdministrativeArea) and the cycle routes (BikeLane); the datasets praha-hrbitovy,
praha-naucne-stezky, praha-vodni-plochy, praha-psi-zony, praha-bilancni-celky and praha-cyklotrasy.

Bento runs each committed mapping over real records downloaded on 2026-10-25
(`fixtures/praha/areas.json`: per layer its three smallest records, one per code where the layer has
a code list, and one named record or one with a web page where the smallest have none). The edge
cases are that answer edited in the one place they name. Each entity is checked against the space's
generated JSON Schema, which the gateway enforces, and the Smart Data Model or extension schema of
its type.
"""

import copy
import json
import re
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-areas")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/areas.json").read_text())
SPACE = "praha-mesto"
# layer -> (IPR item, class, service category, Endpoint, memory quota in MiB)
LAYERS = {
    "cemeteries": ("787f7c3ce4964b519ddc39246377c31b", "PointOfInterest", "cemetery", "hrbitovy", 128),
    "nature-trails": ("927f999ff604463cbce63914de97a6d7", "PointOfInterest", "natureTrail", "naucne-stezky", 128),
    "water-areas": ("e01b4bb6d148496abff4f065686290ff", "PointOfInterest", "waterArea", "vodni-plochy", 512),
    "dog-zones": ("143a034267e34baaacdf253b89f4f09e", "PointOfInterest", "dogZone", "psi-zony", 384),
    "buc": ("8a85aa7854b048f8a5cf8a92070dcbbe", "AdministrativeArea", None, "bilancni-celky", 256),
    "cycle-routes": ("45063acce89d4b37afc6d51f03f3ad49", "BikeLane", None, "cyklotrasy", 384),
}
ERRORED = {"mapping": 'root = if errored() { {"__error": error()} } else { this }'}


def outcome(name: str, document: dict) -> tuple[list[dict], list[str]]:
    """The processors over one answer: the entities written and the errors of the messages the
    runner refuses (an errored message fails the run, nothing is written)."""
    pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-area-{name}-bento.yaml").read_text())
    pipeline["pipeline"]["processors"].append(ERRORED)
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run_all(pinned, json.dumps(document).encode(), SPACE, "praha.eu")
    errors = [m["__error"] for m in answer if "__error" in m]
    return ([] if errors else answer), errors


def run(name: str, document: dict | None = None) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document)
    assert not errors, errors
    return {e["id"].rsplit(f":area-{name}-", 1)[1]: e for e in entities}


def guid(feature: dict) -> str:
    return feature["properties"]["globalid"].strip("{}").lower()


def edited(name: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[name])
    edit(document["features"])
    return document


def sdm(entity: dict) -> list[str]:
    model = {"PointOfInterest": "dataModel.PointOfInterest/PointOfInterest", "BikeLane": "dataModel.Transportation/BikeLane"}.get(
        entity["type"], f"extensions/{entity['type']}")
    return open_data.sdm_errors(model, entity)


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in LAYERS}


@requires_docker
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    for name, (item, kind, category, _, _) in LAYERS.items():
        assert sorted(written[name]) == sorted(guid(f) for f in RECORDED[name]["features"]), name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:area-{name}-")
            assert entity["type"] == kind
            assert entity.get("serviceCategory", {}).get("value") == category
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            assert entity["name"]["languageMap"]["cs"]
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not sdm(entity), (entity["id"], sdm(entity))


@requires_docker
def test_each_record_keeps_its_name_street_web_page_and_geometry(written):
    cemetery = RECORDED["cemeteries"]["features"][0]
    entity = written["cemeteries"][guid(cemetery)]
    assert entity["name"]["languageMap"] == {"cs": cemetery["properties"]["nazev_zar"]}
    assert entity["address"]["value"] == {"streetAddress": cemetery["properties"]["ulice"].strip(), "addressLocality": "Praha", "addressCountry": "CZ"}
    assert entity["location"]["value"]["type"] == "MultiPolygon"
    trails = [e for e in written["nature-trails"].values() if "url" in e]
    assert trails and all(e["url"]["value"].startswith("https://www.praha-priroda.cz/") for e in trails)
    assert {e["location"]["value"]["type"] for e in written["nature-trails"].values()} == {"MultiLineString"}
    buc = RECORDED["buc"]["features"][0]
    assert written["buc"][guid(buc)]["name"]["languageMap"] == {"cs": buc["properties"]["naz_buc"]}


@requires_docker
def test_the_code_list_names_the_kind_and_an_unnamed_area(written):
    water = {e["facilityType"]["value"] for e in written["water-areas"].values()}
    assert water == {"Vodní tok", "Vodní plocha"}
    dogs = {e["facilityType"]["value"] for e in written["dog-zones"].values()}
    assert dogs == {"Volný pohyb psů", "Prosíme, nevstupujte se psy", "Zákaz vstupu psů", "Psi jen na vodítku"}
    routes = {f["properties"]["dopr_stav"] for f in RECORDED["cycle-routes"]["features"]}
    assert len({e["facilityType"]["value"] for e in written["cycle-routes"].values()}) == len(routes) == 25
    for feature in RECORDED["cycle-routes"]["features"]:
        entity = written["cycle-routes"][guid(feature)]
        number = (feature["properties"]["cislo_tras"] or "").strip()
        assert entity["name"]["languageMap"]["cs"] == (f"Cyklotrasa {number}" if number else entity["facilityType"]["value"])
    for name in ("water-areas", "dog-zones"):
        for feature in RECORDED[name]["features"]:
            entity = written[name][guid(feature)]
            own = (feature["properties"]["nazev"] or "").strip()
            assert entity["name"]["languageMap"]["cs"] == (own or entity["facilityType"]["value"])


@requires_docker
def test_outlines_are_rounded_to_6_decimals_without_a_height(written):
    def edit(features):
        ring = features[0]["geometry"]["coordinates"][0][0]
        ring[0] = ring[0][:2] + [235.5]
        ring[-1] = ring[-1][:2] + [235.5]
    areas = run("cemeteries", edited("cemeteries", edit))
    for entity in [*areas.values(), *written["nature-trails"].values(), *written["dog-zones"].values()]:
        text = json.dumps(entity["location"]["value"]["coordinates"])
        assert not re.search(r"\.\d{7,}", text), entity["id"]
        assert "235.5" not in text


@requires_docker
def test_an_unknown_code_an_invalid_or_outside_outline_and_a_duplicate_are_dropped():
    """The owner's plausibility rule (2026-10-22T18:10Z): a code outside the layer's list, a ring that
    is not closed, an outline reaching outside Prague's box and a second record of one GUID are
    dropped and counted; the other areas stay."""
    def edit(features):
        features[0]["properties"]["typ"] = 7
        features[1]["geometry"]["coordinates"][0][0][0] = [16.6, 49.2]
        features[1]["geometry"]["coordinates"][0][0][-1] = [16.6, 49.2]
        features[2]["geometry"]["coordinates"][0][0].pop()
        features.append(copy.deepcopy(features[3]))
        for n in (5, 6):
            more = copy.deepcopy(features[3])
            more["properties"]["globalid"] = "{00000000-0000-4000-8000-00000000000%d}" % n
            features.append(more)
    zones = run("dog-zones", edited("dog-zones", edit))
    kept = [guid(f) for f in RECORDED["dog-zones"]["features"][3:]]
    # Nine read: the unknown code, the outline outside Prague, the open ring and the duplicate dropped.
    assert sorted(zones) == sorted(kept + ["00000000-0000-4000-8000-000000000005", "00000000-0000-4000-8000-000000000006"])


@requires_docker
def test_a_line_of_one_point_and_a_point_in_an_area_layer_are_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"][0] = features[0]["geometry"]["coordinates"][0][:1]
    trails = run("nature-trails", edited("nature-trails", edit))
    assert guid(RECORDED["nature-trails"]["features"][0]) not in trails and len(trails) == len(RECORDED["nature-trails"]["features"]) - 1

    def point(features):
        features[0]["geometry"] = {"type": "Point", "coordinates": [14.4, 50.08]}
    cemeteries = run("cemeteries", edited("cemeteries", point))
    assert guid(RECORDED["cemeteries"]["features"][0]) not in cemeteries


@requires_docker
def test_a_layer_where_fewer_than_half_survive_fails_the_run():
    def edit(features):
        for f in features:
            f["geometry"] = {"type": "Point", "coordinates": [14.4, 50.08]}
    entities, errors = outcome("water-areas", edited("water-areas", edit))
    n = len(RECORDED["water-areas"]["features"])
    assert entities == [] and len(errors) == 1 and f"only 0 of {n} records" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("name", ["buc", "nature-trails"])
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no record"),
    ({"error": {"code": 500, "message": "Internal error"}}, "did not answer a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_layer(name, document, reason):
    entities, errors = outcome(name, document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


def test_each_layer_is_read_weekly_from_its_ipr_download_into_the_city_space():
    for name, (item, kind, _, _, memory) in LAYERS.items():
        source = yaml.safe_load((PRAHA / f"praha-datasource-area-{name}.yaml").read_text())
        assert source["spec"]["http"]["url"] == (
            f"https://hub.arcgis.com/api/v3/datasets/{item}_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1"
        )
        assert "authorization" not in source["spec"]["http"]
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-area-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-area-{name}"
        assert pipeline["spec"]["period"] == "168h"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        text = (PRAHA / f"praha-pipeline-area-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in text and "16777216" in text
        # Water areas (9.0 MB) ran in 320 MiB and not in 256, dog zones (7.1 MB) in 256, cycle routes
        # (5.1 MB) in 256 and not in 192 (2026-10-25).
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] == memory


def test_each_dataset_reads_only_its_own_layer():
    for name, (_, kind, _, endpoint, _) in LAYERS.items():
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{endpoint}.yaml").read_text())
        (selector,) = policy["spec"]["information"][0]["entities"]
        assert selector["type"] == kind
        pattern = re.compile(selector["idPattern"])
        assert pattern.match(f"urn:ngsi-ld:{kind}:praha.eu:praha-mesto:area-{name}-ce91e78f-3923-458b-b680-443b5e3830c0")
        assert not pattern.match(f"urn:ngsi-ld:{kind}:praha.eu:praha-mesto:geo-landmarks-ce91e78f-3923-458b-b680-443b5e3830c0")
        assert policy["spec"]["assignee"] == {"kind": "role", "id": "public"}
        assert not {"create", "update", "delete", "upsert"} & {op.lower() for op in policy["spec"]["operations"]}
        doc = yaml.safe_load((PRAHA / f"praha-endpoint-{endpoint}.yaml").read_text())
        assert doc["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:praha-mesto:public-{endpoint}"
        assert doc["spec"]["catalog"]["license"] == "CC_BY_4_0"
        assert doc["spec"]["catalog"]["personalData"] == "does-not-contain"
        assert doc["spec"]["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": f"area-{name}"}
    # The city amenities keep their geo- points; the districts dataset keeps its 57 districts.
    amenities = yaml.safe_load((PRAHA / "praha-policy-public-vybavenost.yaml").read_text())
    assert not re.match(amenities["spec"]["information"][0]["entities"][0]["idPattern"],
                        "urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:area-cemeteries-ce91e78f-3923-458b-b680-443b5e3830c0")
    districts = yaml.safe_load((PRAHA / "praha-policy-public-mestske-casti.yaml").read_text())
    (selector,) = districts["spec"]["information"][0]["entities"]
    assert re.match(selector["idPattern"], "urn:ngsi-ld:AdministrativeArea:praha.eu:praha-mesto:district-500054")
    assert not re.match(selector["idPattern"], "urn:ngsi-ld:AdministrativeArea:praha.eu:praha-mesto:area-buc-ce91e78f-3923-458b-b680-443b5e3830c0")
