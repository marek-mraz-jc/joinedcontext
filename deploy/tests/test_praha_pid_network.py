"""Prague's public transport network (T-5423, PL-03): the PID lines, stop platforms, metro entrances and
fare zones, as PublicTransportRoute, GtfsStop, GtfsAccessPoint and FareZone of the space praha-mesto,
each its own dataset.

Bento runs each committed mapping over real records read on 2026-10-24 (`fixtures/praha/pid-network.json`:
three lines of the IPR FeatureServer answer, five ROPID platforms, one of them outside Prague, three
metro entrances, and fare zone 0 with the two areas of zone 12). The edge cases are those answers
edited in the one place they name. Each entity is checked against the space's generated JSON Schema,
which the gateway enforces, and the pinned Smart Data Models schema of its type (FareZone has none).
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

pytestmark = pytest.mark.xdist_group("docker-praha-pid-network")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/pid-network.json").read_text())
SPACE = "praha-mesto"
# pipeline -> (type, Smart Data Model, Endpoint file, host of its source)
SETS = {
    "pid-lines": ("PublicTransportRoute", "dataModel.UrbanMobility/PublicTransportRoute", "linky-pid", "mp.iprpraha.cz"),
    "pid-stops": ("GtfsStop", "dataModel.UrbanMobility/GtfsStop", "zastavky-pid", "data.pid.cz"),
    "pid-metro-entrances": ("GtfsAccessPoint", "dataModel.UrbanMobility/GtfsAccessPoint", "vstupy-metra", "hub.arcgis.com"),
    "pid-fare-zones": ("FareZone", None, "tarifni-pasma", "mp.iprpraha.cz"),
}


def outcome(name: str, document: dict) -> tuple[list[dict], list[str]]:
    """The mapping of `name` over one answer: the entities written and the errors of the messages the
    runner refuses (an errored message fails the run, nothing is written)."""
    pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text())
    pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { [{"__error": error()}] } else { this }'})
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run_all(pinned, json.dumps(document).encode(), SPACE, "praha.eu")
    errors = [e["__error"] for e in answer if "__error" in e]
    return ([] if errors else answer), errors


def run(name: str, document: dict | None = None) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document)
    assert not errors, errors
    return {e["id"].rsplit(":", 1)[1]: e for e in entities}


def edited(name: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[name])
    edit(document["features"])
    return document


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in SETS}


@requires_docker
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    assert sorted(written["pid-lines"]) == ["pid-l13", "pid-l59", "pid-l991"]
    # The fifth platform (Králův Dvůr) is outside Prague.
    assert sorted(written["pid-stops"]) == ["pid-u1000z1", "pid-u1000z101", "pid-u100z1", "pid-u685z5"]
    assert sorted(written["pid-metro-entrances"]) == [
        "pid-entrance-649c83da-226f-4d42-9d36-cc26f2710d77",
        "pid-entrance-b1ab7ef5-b4cb-4df2-8ca9-b21ba1c70cc5",
        "pid-entrance-f8329679-78d8-4b55-80a7-8c22a4d0cf97",
    ]
    assert sorted(written["pid-fare-zones"]) == ["pid-zone-0", "pid-zone-12"]
    for name, (kind, model, _, _) in SETS.items():
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:pid-")
            assert entity["type"] == kind
            assert "CC BY 4.0" in entity["dataProvider"]["value"]
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            if model:
                assert not open_data.sdm_errors(model, entity), entity["id"]


@requires_docker
def test_a_line_keeps_its_number_termini_mode_page_and_line(written):
    tram = written["pid-lines"]["pid-l13"]
    assert tram["name"]["languageMap"] == {"cs": "Linka 13"}
    assert (tram["routeCode"]["value"], tram["shortRouteCode"]["value"]) == ("L13", "13")
    assert (tram["transportationType"]["value"], tram["transportMode"]["value"]) == (0, "tram")
    assert tram["description"]["value"] == "Čechovo náměstí - Olšanské hřbitovy"
    assert tram["url"]["value"] == "https://pid.cz/linka/13"
    assert tram["location"]["value"]["type"] == "MultiLineString"
    # GTFS 11 is no Smart Data Model transportationType: a trolleybus is 3, its mode says trolleybus.
    trolleybus = written["pid-lines"]["pid-l59"]
    assert (trolleybus["transportationType"]["value"], trolleybus["transportMode"]["value"]) == (3, "trolleybus")
    assert written["pid-lines"]["pid-l991"]["transportMode"]["value"] == "metro"


@requires_docker
def test_a_stop_keeps_its_platform_zones_lines_modes_and_step_free_access(written):
    stop = written["pid-stops"]["pid-u100z1"]
    assert stop["name"]["languageMap"] == {"cs": "Vltavská"}
    assert stop["platformCode"]["value"] == "A"
    assert stop["servedLines"]["value"] == ["1", "34", "25", "156"]
    assert stop["transportModes"]["value"] == ["tram", "bus"]
    assert stop["wheelchairAccessible"]["value"] is True
    assert written["pid-stops"]["pid-u685z5"]["transportModes"]["value"] == ["bus", "trolleybus"]
    assert written["pid-stops"]["pid-u685z5"]["wheelchairAccessible"]["value"] is False
    border = written["pid-stops"]["pid-u1000z1"]
    # A zone border platform counts in both zones; an unknown step-free access is left out.
    assert border["zoneCode"]["value"] == "P,0"
    assert "wheelchairAccessible" not in border


@requires_docker
def test_an_entrance_names_its_station_sign_way_and_lines(written):
    entrance = written["pid-metro-entrances"]["pid-entrance-b1ab7ef5-b4cb-4df2-8ca9-b21ba1c70cc5"]
    assert entrance["name"]["languageMap"]["cs"].startswith("Můstek – E12")
    assert entrance["code"]["value"] == "E12"
    assert entrance["servedLines"]["value"] == ["A", "B"]
    assert entrance["description"]["value"]


@requires_docker
def test_a_zone_drawn_as_two_areas_is_one_fare_zone_and_the_inner_one_counts_as_p_0_and_b(written):
    outer = written["pid-fare-zones"]["pid-zone-12"]
    parts = [f for f in RECORDED["pid-fare-zones"]["features"] if f["properties"]["id"] == 12]
    assert len(parts) == 2
    assert outer["location"]["value"]["type"] == "MultiPolygon"
    assert len(outer["location"]["value"]["coordinates"]) == sum(len(f["geometry"]["coordinates"]) for f in parts)
    assert (outer["zoneCode"]["value"], outer["name"]["languageMap"]["cs"]) == ("12", "Tarifní pásmo 12")
    inner = written["pid-fare-zones"]["pid-zone-0"]
    assert (inner["zoneCode"]["value"], inner["name"]["languageMap"]["cs"]) == ("P,0,B", "Tarifní pásma P, 0, B")


@requires_docker
def test_an_out_of_area_an_out_of_range_and_a_duplicate_line_are_dropped():
    """The owner's plausibility rule (2026-10-22T18:10Z) on the lines: one vertex at 0,0, a route type
    GTFS does not list and a second record of one route_id are dropped, the rest written."""
    def edit(features):
        twin = copy.deepcopy(features[0])
        features[1]["geometry"]["coordinates"][0][0] = [0, 0]
        features[2]["properties"]["druh"] = 9
        for number in (14, 15):
            other = copy.deepcopy(features[0])
            other["properties"].update({"route_id": f"L{number}", "linka": str(number)})
            features.append(other)
        features.append(twin)
    lines = run("pid-lines", edited("pid-lines", edit))
    assert sorted(lines) == ["pid-l13", "pid-l14", "pid-l15"]


@requires_docker
def test_an_out_of_area_an_out_of_range_and_a_duplicate_stop_are_dropped():
    def edit(features):
        twin = copy.deepcopy(features[0])
        features[1]["properties"]["route_type"] = "1,5"
        features[2]["geometry"]["coordinates"] = [0, 0]
        # An unknown step-free value and a zone outside PID's are left out, the platform kept.
        features[3]["properties"].update({"wheelchair_boarding": "7", "zones_id": "P,Z9"})
        features.append(twin)
    stops = run("pid-stops", edited("pid-stops", edit))
    assert sorted(stops) == ["pid-u1000z1", "pid-u100z1"]
    assert "wheelchairAccessible" not in stops["pid-u1000z1"] and stops["pid-u1000z1"]["zoneCode"]["value"] == "P"


@requires_docker
def test_an_out_of_area_an_out_of_range_and_a_duplicate_entrance_are_dropped():
    def edit(features):
        # A second record of one entrance under another GUID, and one under the same GUID.
        twin = copy.deepcopy(features[1])
        twin["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000001}"
        copied = copy.deepcopy(features[1])
        features[0]["properties"]["vst_linka"] = "A,X"
        features[2]["geometry"]["coordinates"] = [16.6, 49.2]
        for n in (2, 3, 4):
            other = copy.deepcopy(features[1])
            other["properties"].update({"globalid": f"{{00000000-0000-4000-8000-00000000000{n}}}", "vst_nazev": f"E{n}"})
            features.append(other)
        features.extend([twin, copied])
    entrances = run("pid-metro-entrances", edited("pid-metro-entrances", edit))
    assert sorted(entrances) == sorted([f"pid-entrance-00000000-0000-4000-8000-00000000000{n}" for n in (2, 3, 4)]
                                       + ["pid-entrance-f8329679-78d8-4b55-80a7-8c22a4d0cf97"])


@requires_docker
def test_an_out_of_area_an_out_of_range_and_a_duplicate_zone_area_are_dropped():
    def out_of_range_and_duplicate(features):
        features[0]["properties"]["id"] = 13
        features.append(copy.deepcopy(features[2]))
    assert sorted(run("pid-fare-zones", edited("pid-fare-zones", out_of_range_and_duplicate))) == ["pid-zone-0", "pid-zone-12"]

    def outside(features):
        features[1]["geometry"]["coordinates"][0][0][0] = [21.0, 52.2]
    zones = run("pid-fare-zones", edited("pid-fare-zones", outside))
    # Zone 12 keeps the one area left of its two.
    assert len(zones["pid-zone-12"]["location"]["value"]["coordinates"]) == len(RECORDED["pid-fare-zones"]["features"][0]["geometry"]["coordinates"])


@requires_docker
@pytest.mark.parametrize("name", list(SETS))
def test_a_layer_where_fewer_than_half_survive_fails_instead_of_thinning_the_data_out(name):
    def edit(features):
        for feature in features:
            feature["geometry"] = {"type": "Point", "coordinates": [0, 0]} if name in {"pid-stops", "pid-metro-entrances"} else None
    entities, errors = outcome(name, edited(name, edit))
    assert entities == [] and len(errors) == 1 and "only 0 of" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("name", list(SETS))
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no"),
    ({"error": {"code": 500, "message": "Internal error"}}, "did not answer a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_layer(name, document, reason):
    entities, errors = outcome(name, document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


@requires_docker
@pytest.mark.parametrize("name", ["pid-lines", "pid-fare-zones"])
def test_an_answer_the_server_cut_at_its_record_limit_fails_the_run(name):
    document = copy.deepcopy(RECORDED[name]) | {"exceededTransferLimit": True}
    entities, errors = outcome(name, document)
    assert entities == [] and len(errors) == 1 and "record limit" in errors[0], errors


def test_each_pipeline_reads_a_fixed_https_source_without_a_key_into_the_city_space():
    for name, (kind, _, _, host) in SETS.items():
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        source = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert source["metadata"]["name"] == pipeline["spec"]["source"]["dataSourceRef"]["name"]
        assert re.match(rf"https://{re.escape(host)}/", source["spec"]["http"]["url"]), name
        assert "authorization" not in source["spec"]["http"]
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        assert "content().length() > 16777216" in mapping, name
    # The layers the FeatureServer simplifies to about 5 m, so the lines fit under the 16 MiB cap.
    for name in ("pid-lines", "pid-fare-zones"):
        url = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())["spec"]["http"]["url"]
        assert "maxAllowableOffset=0.00005" in url and "outSR=4326" in url and "f=geojson" in url


def test_each_type_is_its_own_public_cc_by_dataset_and_its_policy_names_only_its_ids():
    ids = {
        "PublicTransportRoute": f"urn:ngsi-ld:PublicTransportRoute:praha.eu:{SPACE}:pid-l991",
        "GtfsStop": f"urn:ngsi-ld:GtfsStop:praha.eu:{SPACE}:pid-u425z2p",
        "GtfsAccessPoint": f"urn:ngsi-ld:GtfsAccessPoint:praha.eu:{SPACE}:pid-entrance-f8329679-78d8-4b55-80a7-8c22a4d0cf97",
        "FareZone": f"urn:ngsi-ld:FareZone:praha.eu:{SPACE}:pid-zone-12",
    }
    public_read = yaml.safe_load((PRAHA / "praha-policy-public-read.yaml").read_text())
    read_types = {e["type"] for e in public_read["spec"]["information"][0]["entities"]}
    for kind, _, dataset, _ in SETS.values():
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{dataset}.yaml").read_text())
        assert endpoint["spec"]["policyRef"].endswith(f":public-{dataset}")
        assert endpoint["spec"]["audience"] == "public"
        assert endpoint["spec"]["catalog"]["license"] == "CC_BY_4_0"
        assert endpoint["spec"]["publish"]["ckan"]["instanceRef"]["name"] == "praha"
        (selector,) = yaml.safe_load((PRAHA / f"praha-policy-public-{dataset}.yaml").read_text())["spec"]["information"][0]["entities"]
        assert selector["type"] == kind
        assert {k for k, i in ids.items() if re.fullmatch(selector["idPattern"], i)} == {kind}, dataset
        assert kind in read_types
