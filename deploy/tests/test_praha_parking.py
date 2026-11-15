"""Prague's street parking from the TSK layers of the IPR Praha geoportal (T-5424, PL-03): the paid
parking sections and their street segments, the bays reserved for ZTP/P card holders, deliveries and
special purposes (OnStreetParking, ParkingGroup), the no-parking stretches (RestrictedTrafficArea) and
the parking machines (FareCollectionSystem), published as the dataset praha-parkovani.

Bento runs each committed mapping over real records downloaded from hub.arcgis.com on 2026-10-24
(`fixtures/praha/ipr-parking.json`, the first three features of each layer; the segments are the
three of the fixture's sections). The edge cases are that answer edited in the one place they name.
Each entity is checked against the space's generated JSON Schema, which the gateway enforces, and the
pinned Smart Data Models schema of its type.
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

pytestmark = pytest.mark.xdist_group("docker-praha-parking")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/ipr-parking.json").read_text())
SPACE = "praha-mesto"
SDM = {
    "OnStreetParking": "dataModel.Parking/OnStreetParking",
    "ParkingGroup": "dataModel.Parking/ParkingGroup",
    "RestrictedTrafficArea": "dataModel.Transportation/RestrictedTrafficArea",
    "FareCollectionSystem": "dataModel.Transportation/FareCollectionSystem",
}
LAYERS = {
    "zps-sections": ("8e3420ecc539468489101958689a3cd9", "OnStreetParking", None),
    "zps-segments": ("7cbfdf9f9d87418ba62b35762cd46ca7", "ParkingGroup", None),
    "ztp-bays": ("ab4a8cd8fc284b738ca806681559d694", "OnStreetParking", ["forDisabled"]),
    "ztp-shared": ("6bceef0c12d84031a0628fab533eecc1", "OnStreetParking", ["forDisabled"]),
    "ztp-plate": ("1e6b9ad51a12468193f47e2b9b1b94ed", "OnStreetParking", ["forDisabled", "onlyWithPermit"]),
    "delivery": ("b62d8490d0034b55b2982386a5d1027c", "OnStreetParking", ["forLoadUnload"]),
    "special": ("720a5804b9ef4c73af2d85f5605c1dc6", "OnStreetParking", ["onlyWithPermit"]),
    "no-parking": ("b2be4777cec1443fa1e9b4b0d385fed1", "RestrictedTrafficArea", None),
    "machines": ("2b49ad1ad0bb4892bcb54acfb978456a", "FareCollectionSystem", None),
}


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


def key(name: str, feature: dict) -> str:
    if name == "zps-sections":
        return feature["properties"]["code"].lower()
    if name == "machines":
        return feature["properties"]["pa"]
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
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    for name, (item, kind, category) in LAYERS.items():
        assert sorted(written[name]) == sorted(key(name, f) for f in RECORDED[name]["features"]), name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:geo-{name}-")
            assert entity["type"] == kind
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha, TSK hl. m. Prahy (CC BY 4.0)"
            if category is not None:
                assert entity["category"]["value"] == category
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not open_data.sdm_errors(SDM[kind], entity), entity["id"]


@requires_docker
def test_a_section_carries_its_zone_tariff_and_spaces_and_a_segment_points_at_its_section(written):
    section = written["zps-sections"]["bus-0001"]
    assert section["name"]["languageMap"] == {"cs": "Úsek BUS-0001"}
    assert section["parkingZone"]["value"] == "VIS"
    assert section["category"]["value"] == ["feeCharged", "shortTerm"]
    assert section["tariff"]["value"] == "Po-Ne 08:00-19:59 300Kč/hod (max. 75 Kč)"
    assert section["totalSpotNumber"]["value"] == 3
    assert section["location"]["value"]["type"] == "MultiPolygon"
    segment = written["zps-segments"][key("zps-segments", RECORDED["zps-segments"]["features"][0])]
    assert segment["refParkingSite"] == {
        "type": "Relationship", "object": f"urn:ngsi-ld:OnStreetParking:praha.eu:{SPACE}:geo-zps-sections-bus-0001"}
    assert segment["totalSpotNumber"]["value"] == 3
    # Every segment's section is a section the same answer of the sections layer writes.
    sections = {e["id"] for e in written["zps-sections"].values()}
    assert {e["refParkingSite"]["object"] for e in written["zps-segments"].values()} <= sections


@requires_docker
@pytest.mark.parametrize("zone,category", [
    ("RES", ["forResidents", "onlyWithPermit"]), ("MIX", ["feeCharged", "forResidents"]), ("VIS", ["feeCharged", "shortTerm"])])
def test_each_zone_is_its_sdm_category(zone, category):
    def edit(features):
        for feature in features:
            feature["properties"]["category"] = zone
    for entity in run("zps-sections", edited("zps-sections", edit)).values():
        assert entity["category"]["value"] == category and entity["parkingZone"]["value"] == zone


@requires_docker
def test_an_unknown_zone_or_a_segment_without_a_tariff_code_is_dropped():
    def zone(features):
        features[0]["properties"]["category"] = "XYZ"
    assert sorted(run("zps-sections", edited("zps-sections", zone))) == ["bus-0002", "bus-0003"]

    def code(features):
        features[0]["properties"]["tariftab"] = " "
        # A code the layer writes in lower case names the same section.
        features[1]["properties"]["tariftab"] = "bus-0003"
    segments = run("zps-segments", edited("zps-segments", code))
    first, second, _ = (key("zps-segments", f) for f in RECORDED["zps-segments"]["features"])
    assert first not in segments
    assert segments[second]["refParkingSite"]["object"].endswith(":geo-zps-sections-bus-0003")


@requires_docker
def test_a_machine_keeps_its_number_and_its_street_without_the_segment_code(written):
    machine = written["machines"]["22000004"]
    assert machine["equipmentId"]["value"] == "22000004"
    assert machine["name"]["languageMap"] == {"cs": "Parkovací automat 22000004"}
    assert machine["address"]["value"] == {"streetAddress": "Františka Diviše", "addressLocality": "Praha", "addressCountry": "CZ"}
    assert machine["location"]["value"]["type"] == "Point"
    assert len(machine["location"]["value"]["coordinates"]) == 2


@requires_docker
def test_out_of_area_zero_and_swapped_points_fail_a_layer_where_fewer_than_half_survive():
    """The owner's plausibility rule (2026-10-22T18:10Z): a point at 0,0, one with latitude and
    longitude swapped and one outside Prague are dropped; with none of three left the run fails
    instead of thinning the layer out."""
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["geometry"]["coordinates"] = list(reversed(features[1]["geometry"]["coordinates"]))
        features[2]["geometry"]["coordinates"] = [16.6, 49.2]
    entities, errors = outcome("machines", edited("machines", edit))
    assert entities == [] and len(errors) == 1 and "only 0 of 3 records" in errors[0], errors


@requires_docker
def test_an_area_with_one_vertex_outside_prague_an_open_ring_or_a_short_ring_is_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"][0][0][1] = [16.6, 49.2]
        opened = copy.deepcopy(features[2])
        opened["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000001}"
        ring = opened["geometry"]["coordinates"][0][0]
        ring[-1] = [ring[-1][0] + 0.001, ring[-1][1]]
        features.append(opened)
    areas = run("no-parking", edited("no-parking", edit))
    assert sorted(areas) == sorted(key("no-parking", f) for f in RECORDED["no-parking"]["features"][1:])

    def short(features):
        ring = features[0]["geometry"]["coordinates"][0][0]
        features[0]["geometry"]["coordinates"][0][0] = [ring[0], ring[1], ring[0]]
    areas = run("no-parking", edited("no-parking", short))
    assert key("no-parking", RECORDED["no-parking"]["features"][0]) not in areas and len(areas) == 2


@requires_docker
@pytest.mark.parametrize("count", ["4000", "1.5", "-1", "dva"])
def test_a_space_count_out_of_range_or_not_whole_is_dropped(count):
    bays = run("ztp-shared", edited("ztp-shared", lambda f: f[0]["properties"].update(pocinvst=count)))
    assert sorted(bays) == sorted(key("ztp-shared", f) for f in RECORDED["ztp-shared"]["features"][1:])


@requires_docker
def test_a_duplicate_guid_or_a_duplicate_area_is_kept_once():
    def edit(features):
        twin = copy.deepcopy(features[2])
        twin["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000002}"
        same = copy.deepcopy(features[2])
        same["geometry"]["coordinates"][0][0][0][0] += 0.00001
        features.extend([twin, same])
    bays = run("ztp-shared", edited("ztp-shared", edit))
    # The twin repeats the third bay's area under another GUID; the copy with the third bay's GUID
    # and a moved vertex is the same bay listed twice.
    assert sorted(bays) == sorted(key("ztp-shared", f) for f in RECORDED["ztp-shared"]["features"])
    assert bays[key("ztp-shared", RECORDED["ztp-shared"]["features"][2])]["totalSpotNumber"]["value"] == 2


@requires_docker
def test_a_height_coordinate_is_dropped_and_a_polygon_becomes_a_multipolygon():
    def edit(features):
        features[0]["geometry"]["coordinates"] = features[0]["geometry"]["coordinates"] + [235.0]
        features[1]["geometry"] = {"type": "Polygon", "coordinates": [[[14.40, 50.08, 1.0], [14.41, 50.08, 1.0], [14.41, 50.09, 1.0], [14.40, 50.08, 1.0]]]}
    bays = run("ztp-bays", edited("ztp-bays", edit))
    assert len(bays[key("ztp-bays", RECORDED["ztp-bays"]["features"][0])]["location"]["value"]["coordinates"]) == 2
    # A bay layer of points takes no area: the polygon is dropped.
    assert key("ztp-bays", RECORDED["ztp-bays"]["features"][1]) not in bays
    delivery = run("delivery", edited("delivery", lambda f: f[0].update(geometry={
        "type": "Polygon", "coordinates": [[[14.40, 50.08, 1.0], [14.41, 50.08, 1.0], [14.41, 50.09, 1.0], [14.40, 50.08, 1.0]]]})))
    area = delivery[key("delivery", RECORDED["delivery"]["features"][0])]["location"]["value"]
    assert area == {"type": "MultiPolygon", "coordinates": [[[[14.40, 50.08], [14.41, 50.08], [14.41, 50.09], [14.40, 50.08]]]]}


@requires_docker
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no record"),
    ({"error": {"code": 500, "message": "Internal error"}}, "did not answer a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_layer(document, reason):
    entities, errors = outcome("zps-segments", document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


def test_each_pipeline_reads_its_ipr_layer_into_the_city_space_weekly():
    for name, (item, kind, _) in LAYERS.items():
        source = yaml.safe_load((PRAHA / f"praha-datasource-geo-{name}.yaml").read_text())
        assert source["spec"]["http"]["url"] == (
            f"https://hub.arcgis.com/api/v3/datasets/{item}_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1"
        )
        assert "authorization" not in source["spec"]["http"]
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-geo-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-geo-{name}"
        assert pipeline["spec"]["period"] == "168h"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        # Measured with docker run --memory over the whole layer of 2026-10-24: the 11.6 MB segments
        # layer needs 384 MiB, the 7.2 MB no-parking one 256 and the 4.9 MB sections 128.
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 512
        mapping = (PRAHA / f"praha-pipeline-geo-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in mapping and f'let set = "{name}"' in mapping
    # The segments name their sections: they run after the sections' weekly read.
    schedule = {n: yaml.safe_load((PRAHA / f"praha-pipeline-geo-{n}.yaml").read_text())["spec"]["schedule"] for n in ("zps-sections", "zps-segments")}
    assert schedule["zps-sections"] < schedule["zps-segments"]


# One dataset per Smart Data Model type or referencing group (T-4344): the sections and their
# segments with the reserved bays, the no-parking stretches, the machines.
DATASETS = {
    "parkovani": ({"OnStreetParking", "ParkingGroup"}, 7),
    "zakazy-stani": ({"RestrictedTrafficArea"}, 1),
    "parkovaci-automaty": ({"FareCollectionSystem"}, 1),
}


def test_the_parking_layers_are_their_own_datasets_and_no_other_policy_reads_them():
    selectors = {}
    for name, (types, sources) in DATASETS.items():
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{name}.yaml").read_text())
        found = {s["type"]: s["idPattern"] for s in policy["spec"]["information"][0]["entities"]}
        assert set(found) == types, name
        selectors.update(found)
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{name}.yaml").read_text())
        assert endpoint["spec"]["policyRef"].endswith(f":public-{name}")
        assert endpoint["spec"]["catalog"]["license"] == "CC_BY_4_0"
        assert len(endpoint["spec"]["catalog"]["source"]) == sources, name
    assert set(selectors) == set(SDM)
    ids = {
        "OnStreetParking": [f"urn:ngsi-ld:OnStreetParking:praha.eu:{SPACE}:geo-zps-sections-p1-0241",
                            f"urn:ngsi-ld:OnStreetParking:praha.eu:{SPACE}:geo-ztp-bays-b0af6b19-b810-4fb9-bd7f-cf8508f0c768"],
        "ParkingGroup": [f"urn:ngsi-ld:ParkingGroup:praha.eu:{SPACE}:geo-zps-segments-82c6727e-b9cd-419a-820e-b689aef0da8b"],
        "RestrictedTrafficArea": [f"urn:ngsi-ld:RestrictedTrafficArea:praha.eu:{SPACE}:geo-no-parking-29247e35-861c-408c-b753-0b6fad2e34dc"],
        "FareCollectionSystem": [f"urn:ngsi-ld:FareCollectionSystem:praha.eu:{SPACE}:geo-machines-8000114"],
    }
    for kind, examples in ids.items():
        assert all(re.fullmatch(selectors[kind], i) for i in examples), kind
    # Another space's id or another prefix is not read through these Policies.
    assert not re.fullmatch(selectors["OnStreetParking"], "urn:ngsi-ld:OnStreetParking:praha.eu:other:geo-ztp-bays-x")
    assert not re.fullmatch(selectors["FareCollectionSystem"], f"urn:ngsi-ld:FareCollectionSystem:praha.eu:{SPACE}:golemio-1")
    mine = {f"praha-policy-public-{n}.yaml" for n in DATASETS} | {"praha-policy-public-read.yaml", "praha-policy-public-read-history.yaml"}
    for other in PRAHA.glob("praha-policy-public-*.yaml"):
        if other.name in mine:
            continue
        types = {s.get("type") for b in yaml.safe_load(other.read_text())["spec"].get("information", []) for s in b.get("entities", [])}
        assert not types & set(SDM), other.name
