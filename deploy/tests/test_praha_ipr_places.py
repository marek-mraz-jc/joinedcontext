"""Prague's city amenities from the IPR Praha geoportal (T-5269, PL-03): health centres, universities,
places of worship, community centres, accommodation, grocery stores, cool spots, animal corners and
landmarks, one PointOfInterest each, published as the dataset praha-vybavenost.

Bento runs each committed mapping over real records downloaded from hub.arcgis.com on 2026-10-23
(`fixtures/praha/ipr-places.json`, the first three named features of each layer; the animal corners
without the contact person, e-mail and phone the mapping never reads). The edge cases are that answer
edited in the one place they name. Each entity is checked against the space's generated JSON Schema,
which the gateway enforces, and the pinned Smart Data Models PointOfInterest schema.
"""

import copy
import json
import re
import shutil
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-ipr-places")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/ipr-places.json").read_text())
SPACE = "praha-mesto"
LAYERS = {
    "health-centres": ("3fbd520692be4ac889550cf7c5137282", "healthCentre"),
    "universities": ("28a9425f2ee748328fc5346030de2d83", "university"),
    "places-of-worship": ("47befb94777e42808f9934f60768072b", "placeOfWorship"),
    "community-centres": ("c0bca952f09d44b0bf38885f499ad59d", "communityCentre"),
    "accommodation": ("81c543a1199e46da87e18e12125e29a3", "accommodation"),
    "grocery-stores": ("97a5ee3b3d0c4e27b423f28daeb84e67", "groceryStore"),
    "drinking-fountains": ("ae0ef46774394e09a91850fb9b826788", "drinkingFountain"),
    "fountains": ("3d3add84fb784ed297725c566f149a51", "fountain"),
    "springs": ("8aabe75c9689487a88d3e297bf498d6a", "spring"),
    "water-pumps": ("39f6228757cf424889f79b0ef7a7b415", "waterPump"),
    "mist-showers": ("33eadb4efd7944adb3abf5cb77b40c06", "mistShower"),
    "bathing-places": ("8d435d33cd5d431cbaeda5837b3d3a77", "bathingPlace"),
    "animal-corners": ("e332ec5438b645b6a08a54a0da1accd6", "animalCorner"),
    "landmarks": ("fb6ab7891a7244f39a3a380027af2981", "landmark"),
    "waste-points": ("176f314a9ded40ef88ce55a2ed401c2e", "wasteCollectionPoint"),
    "trail-stops": ("bed7894d358a4d08b640c972e844d7f3", "trailStop"),
}


def yard(feature: dict) -> bool:
    """A collection yard of the waste facilities layer, which golemio-yards writes from Golemio."""
    return feature["properties"].get("typobjektu", "").startswith("SD")


def outcome(name: str, document: dict) -> tuple[list[dict], list[str]]:
    """The mapping of `geo-<name>` over one answer: the entities written and the error of a message
    the runner refuses (an errored message fails the run, nothing is written)."""
    pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-geo-{name}-bento.yaml").read_text())
    pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run(pinned, json.dumps(document).encode(), SPACE, "praha.eu")
    if isinstance(answer, dict) and "__error" in answer:
        return [], [answer["__error"]]
    return answer, []


def run(name: str, document: dict | None = None) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document)
    assert not errors, errors
    return {e["id"].rsplit(f":geo-{name}-", 1)[1]: e for e in entities}


def guid(feature: dict) -> str:
    return feature["properties"]["globalid"].strip("{}").lower()


def edited(name: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[name])
    edit(document["features"])
    return document


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in LAYERS}


@requires_docker
def test_every_record_is_one_point_of_interest_the_gateway_and_the_sdm_accept(written):
    for name, (item, category) in LAYERS.items():
        assert sorted(written[name]) == sorted(guid(f) for f in RECORDED[name]["features"] if not yard(f)), name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:geo-{name}-")
            assert entity["serviceCategory"]["value"] == category
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            assert len(entity["location"]["value"]["coordinates"]) == 2
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity), entity["id"]


@requires_docker
def test_name_address_type_web_and_season_come_from_the_layer_fields(written):
    centre = written["community-centres"][guid(RECORDED["community-centres"]["features"][0])]
    assert centre["name"]["languageMap"] == {"cs": "Místní knihovna Praha – Běchovice"}
    assert centre["facilityType"]["value"] == "knihovna"
    assert centre["address"]["value"]["streetAddress"] == "Za poštovskou zahradou 479"
    # A web address without a scheme gets https.
    assert centre["url"]["value"] == "https://www.bechovice.knihovna.cz/"
    hotel = written["accommodation"][guid(RECORDED["accommodation"]["features"][0])]
    # adresa up to its first comma: the district and postcode stay out of streetAddress.
    assert hotel["address"]["value"] == {"streetAddress": "Pobřežní 311/1", "addressLocality": "Praha", "addressCountry": "CZ"}
    store = written["grocery-stores"][guid(RECORDED["grocery-stores"]["features"][0])]
    # ulice with the house numbers when the layer has no address text.
    assert store["address"]["value"]["streetAddress"] == "Prosecká 824/82"
    assert store["facilityType"]["value"] == "Penny Market"
    stop = written["trail-stops"][guid(RECORDED["trail-stops"]["features"][0])]
    assert stop["name"]["languageMap"] == {"cs": "Roztocký háj"}
    point = written["waste-points"][guid(RECORDED["waste-points"]["features"][0])]
    assert point["address"]["value"]["streetAddress"] == "Bartoškova 1/a"
    shower = written["mist-showers"][guid(RECORDED["mist-showers"]["features"][0])]
    assert shower["openingHours"]["languageMap"] == {"cs": "květen - září"}


@requires_docker
def test_a_numeric_code_and_the_contact_details_are_not_read(written):
    for entity in written["fountains"].values():
        # typ is a number whose code list IPR does not publish.
        assert "facilityType" not in entity
    document = copy.deepcopy(RECORDED["animal-corners"])
    document["features"][0]["properties"].update({"kontakt": "Jana Nováková", "email": "jana@example.org", "telefon": "222333444"})
    (corner,) = [e for k, e in run("animal-corners", document).items() if k == guid(document["features"][0])]
    assert not {"Jana", "jana@example.org", "222333444"} & set(re.findall(r"[\w.@]+", json.dumps(corner, ensure_ascii=False)))


@requires_docker
def test_out_of_area_zero_and_swapped_points_fail_a_layer_where_fewer_than_half_survive():
    """The owner's plausibility rule (2026-10-22T18:10Z): a point at 0,0, one with latitude and
    longitude swapped and one outside Prague are dropped; with none of three left the run fails
    instead of thinning the layer out."""
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["geometry"]["coordinates"] = list(reversed(features[1]["geometry"]["coordinates"]))
        features[2]["geometry"]["coordinates"] = [16.6, 49.2]
    entities, errors = outcome("springs", edited("springs", edit))
    assert entities == [] and len(errors) == 1 and "only 0 of 3 records" in errors[0], errors


@requires_docker
def test_a_duplicate_within_15_m_and_an_unnamed_record_are_dropped():
    def edit(features):
        twin = copy.deepcopy(features[0])
        twin["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000002}"
        lon, lat = features[0]["geometry"]["coordinates"][:2]
        twin["geometry"]["coordinates"] = [lon + 0.00005, lat]
        unnamed = copy.deepcopy(features[2])
        unnamed["properties"].update({"nazev": "  ", "globalid": "{00000000-0000-4000-8000-000000000003}"})
        features.extend([twin, unnamed])
    springs = run("springs", edited("springs", edit))
    # Five read, the twin of the first (about 4 m east) and the unnamed record dropped.
    assert sorted(springs) == sorted(guid(f) for f in RECORDED["springs"]["features"])


@requires_docker
def test_the_collection_yards_golemio_yards_writes_are_left_out_and_do_not_count_as_failures():
    """One yard is one entity: the yards (SDHMP, SDMC, SDMC_SSNO) come from golemio-yards. Three
    yards beside three collection points are not "fewer than half" of the layer."""
    def edit(features):
        extra = copy.deepcopy(next(f for f in features if yard(f)))
        extra["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000004}"
        extra["properties"]["typobjektu"] = "SDMC_SSNO"
        features.append(extra)
    points = run("waste-points", edited("waste-points", edit))
    assert sorted(points) == sorted(guid(f) for f in RECORDED["waste-points"]["features"] if not yard(f))
    assert len(points) == 3 and sum(map(yard, RECORDED["waste-points"]["features"])) == 2


@requires_docker
def test_a_height_coordinate_is_dropped_and_a_non_ascii_web_address_left_out():
    def edit(features):
        features[0]["geometry"]["coordinates"] = features[0]["geometry"]["coordinates"][:2] + [0.0]
        features[1]["properties"]["web"] = "https://www.mlp.cz/cz/kontakty/pobocky/zahradni-město"
        features[2]["properties"]["web"] = "javascript:alert(1)"
    centres = run("community-centres", edited("community-centres", edit))
    first, second, third = (guid(f) for f in RECORDED["community-centres"]["features"])
    assert len(centres[first]["location"]["value"]["coordinates"]) == 2
    assert "url" not in centres[second] and "url" not in centres[third]
    assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", centres[second])


@requires_docker
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no record"),
    ({"error": {"code": 500, "message": "Internal error"}}, "did not answer a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_places(document, reason):
    entities, errors = outcome("landmarks", document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


def test_each_pipeline_reads_its_ipr_layer_into_the_city_space_weekly():
    for name, (item, _) in LAYERS.items():
        source = yaml.safe_load((PRAHA / f"praha-datasource-geo-{name}.yaml").read_text())
        assert source["spec"]["http"]["url"] == (
            f"https://hub.arcgis.com/api/v3/datasets/{item}_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1"
        )
        assert "authorization" not in source["spec"]["http"]
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-geo-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-geo-{name}"
        assert pipeline["spec"]["period"] == "168h"
        assert pipeline["spec"]["output"] == {"type": "PointOfInterest", "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        mapping = (PRAHA / f"praha-pipeline-geo-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in mapping


def test_the_ipr_amenities_are_their_own_dataset_beside_the_other_places():
    selectors = {
        name: yaml.safe_load((PRAHA / f"praha-policy-public-{name}.yaml").read_text())["spec"]["information"][0]["entities"][0]["idPattern"]
        for name in ("mista", "sluzby", "vybavenost")
    }
    ids = {
        "vybavenost": f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:geo-health-centres-a3a83aa6-a528-4ded-958c-7f5867917a77",
        "mista": f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:ipr-0b1c2d3e-0000-4000-8000-000000000000",
        "sluzby": f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:golemio-libraries-barrandov",
    }
    for policy, pattern in selectors.items():
        assert {name for name, entity in ids.items() if re.fullmatch(pattern, entity)} == {policy}, policy
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-vybavenost.yaml").read_text())
    assert endpoint["spec"]["policyRef"].endswith(":public-vybavenost")
    assert endpoint["spec"]["catalog"]["license"] == "CC_BY_4_0"
    assert len(endpoint["spec"]["catalog"]["source"]) == len(LAYERS)
