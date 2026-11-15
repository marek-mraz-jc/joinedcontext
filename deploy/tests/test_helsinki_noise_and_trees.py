"""T-5378: the city's 2022 noise survey and its street and park tree register from kartta.hel.fi in the
Helsinki space (PL-03, DM-61).

Bento runs each committed mapping over the WFS answers the deployer recorded on 2026-10-26 (T-5668,
`fixtures/helsinki/kartta/`), and every entity it writes is checked against the space's generated JSON
Schema and, for NoiseZone, its Smart Data Model extension (Tree has no Smart Data Model). The edge
cases are those answers edited in the one place they name.

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

pytestmark = pytest.mark.xdist_group("docker-helsinki-noise-trees")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
KARTTA = Path(__file__).resolve().parent / "fixtures/helsinki/kartta"
WFS = ("https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&version=2.0.0&request=GetFeature"
       "&outputFormat=application/json&srsName=EPSG:4326&typeNames=")
SURVEY = "Meluselvitys_2022_Helsinki_{}_LAeq_{}_NPM"
ROADS = {"day": SURVEY.format("kadut_ja_maantiet", "7-22"), "night": SURVEY.format("kadut_ja_maantiet", "22-7")}
RAIL = [SURVEY.format(source, hours) for source in ("juna", "metro", "raitiovaunu") for hours in ("7-22", "22-7")]
NOISE = [f"noise-roads-{period}-{i}" for period in ("day", "night") for i in range(3)] + ["noise-rail"]
TREES = [f"trees-{i}" for i in range(4)]
TREE_LAYER = "Puurekisteri_piste"
# Turns a failed run into one message naming its error, as the runner's log reads it.
FAILED = ({"catch": [{"mapping": 'root = {"failed": error()}'}]},)


def recording(layer: str) -> dict:
    return json.loads((KARTTA / f"{layer}.json").read_text())


def rail() -> dict:
    """The six rail layers in one answer, as GeoServer gives them for typeNames=(a)(b)..."""
    layers = [recording(layer) for layer in RAIL]
    return {**layers[0], "features": [f for layer in layers for f in layer["features"]]}


def run(pipeline: str, document: dict) -> list[dict]:
    return open_data.run_all(HELSINKI / f"helsinki-pipeline-{pipeline}-bento.yaml", json.dumps(document).encode(),
                             "helsinki", then=FAILED)


def failure(pipeline: str, document: dict) -> str:
    out = run(pipeline, document)
    assert len(out) == 1 and "failed" in out[0], out[:1]
    return out[0]["failed"]


@pytest.fixture(scope="module")
def written():
    return {
        "roads-day": run("noise-roads-day-0", recording(ROADS["day"])),
        "roads-night": run("noise-roads-night-0", recording(ROADS["night"])),
        "rail": run("noise-rail", rail()),
        "trees": run("trees-0", recording(TREE_LAYER)),
    }


@requires_docker
def test_each_recorded_polygon_and_tree_is_written_once_and_the_gateway_accepts_it(written):
    counts = {"roads-day": 20, "roads-night": 20, "rail": sum(len(recording(layer)["features"]) for layer in RAIL),
              "trees": 20}
    for name, entities in written.items():
        assert len(entities) == counts[name], name
        assert len({e["id"] for e in entities}) == counts[name], name
        kind = "Tree" if name == "trees" else "NoiseZone"
        for entity in entities:
            assert entity["type"] == kind
            assert re.fullmatch(rf"urn:ngsi-ld:{kind}:hel\.fi:helsinki:[A-Za-z0-9-]+", entity["id"]), entity["id"]
            assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], (entity["id"],)
            if kind == "NoiseZone":
                assert open_data.sdm_errors("extensions/NoiseZone", entity) == [], entity["id"]
            assert entity["dataProvider"]["value"]
            assert entity["source"]["value"].startswith("https://kartta.hel.fi/ws/geoserver/avoindata/wfs?typeNames=")
            assert entity["expiresAt"]


@requires_docker
def test_a_noise_area_takes_its_source_period_and_band_from_its_layer(written):
    layer = recording(ROADS["night"])
    feature = layer["features"][0]
    area = next(e for e in written["roads-night"] if e["id"].endswith(f"-{feature['properties']['id']}"))
    lo, hi = feature["properties"]["db_lo"], feature["properties"]["db_hi"]
    assert area["id"] == f"urn:ngsi-ld:NoiseZone:hel.fi:helsinki:noise-roadTraffic-night-{feature['properties']['id']}"
    assert area["noiseSource"]["value"] == "roadTraffic" and area["assessmentPeriod"]["value"] == "night"
    assert area["noiseLevelLow"] == {"type": "Property", "value": lo, "unitCode": "2N"}
    assert area["noiseLevelHigh"]["value"] == hi == lo + 5
    assert area["name"]["languageMap"] == {"fi": f"Katu- ja tieliikenne, yö 22–7: {lo}–{hi} dB",
                                           "en": f"Road traffic, night 22–7: {lo}–{hi} dB"}
    assert area["location"]["value"] == feature["geometry"]
    sources = {(e["noiseSource"]["value"], e["assessmentPeriod"]["value"]) for e in written["rail"]}
    assert sources == {(s, p) for s in ("railway", "metro", "tram") for p in ("day", "night")}
    loudest = copy.deepcopy(feature)
    loudest["properties"].update({"db_lo": 75, "db_hi": None})
    out = run("noise-roads-night-0", {**layer, "features": [loudest]})
    assert "noiseLevelHigh" not in out[0]
    assert out[0]["name"]["languageMap"]["en"] == "Road traffic, night 22–7: 75 dB or more"


@requires_docker
def test_a_noise_polygon_of_an_unknown_layer_band_or_without_an_outline_is_not_written():
    layer = recording(ROADS["day"])
    good = layer["features"][:10]
    bad = []
    for change in ({"id": "Meluselvitys_2022_Helsinki_yhteismelu_LAeg_7-22_NPM.1"},
                   {"properties": {"db_lo": 50, "db_hi": 60}},
                   {"properties": {"db_lo": "kova"}},
                   {"geometry": None},
                   {"geometry": {"type": "Polygon", "coordinates": [[[13.4, 52.5], [13.5, 52.5], [13.5, 52.6], [13.4, 52.5]]]}}):
        feature = copy.deepcopy(good[0])
        for key, value in change.items():
            if key == "properties":
                feature["properties"].update(value)
            else:
                feature[key] = value
        bad.append(feature)
    out = run("noise-roads-day-0", {**layer, "features": good + bad + [copy.deepcopy(good[1])]})
    assert sorted(e["id"] for e in out) == sorted(
        f"urn:ngsi-ld:NoiseZone:hel.fi:helsinki:noise-roadTraffic-day-{f['properties']['id']}" for f in good)


@requires_docker
def test_an_answer_that_is_mostly_unusable_or_no_feature_list_fails_the_run():
    layer = recording(ROADS["day"])
    moved = copy.deepcopy(layer)
    for feature in moved["features"][:11]:
        feature["geometry"] = None
    assert "only 9 of 20 noise polygons" in failure("noise-roads-day-0", moved)
    assert "no polygon" in failure("noise-rail", {"type": "FeatureCollection", "features": []})
    assert "no tree" in failure("trees-0", {"exceptionCode": "OperationProcessingFailed"})


@requires_docker
def test_a_tree_carries_its_names_trunk_class_setting_and_site(written):
    feature = recording(TREE_LAYER)["features"][0]["properties"]
    tree = next(e for e in written["trees"] if e["id"] == f"urn:ngsi-ld:Tree:hel.fi:helsinki:tree-{feature['tunnus']}")
    assert feature["tunnus"] == "K-929" and feature["suomenknimi"] == "VUORIJALAVA"
    assert tree["name"]["languageMap"] == {"fi": "Vuorijalava"}
    assert tree["species"]["value"] == "Ulmus glabra"
    assert tree["trunkDiameterClass"]["value"] == "cm10to20"
    assert tree["treeSetting"]["value"] == "street" and tree["treeSite"]["value"] == "Ratapihantie"
    assert tree["location"]["value"]["type"] == "Point"
    assert all(e["treeSetting"]["value"] in ("street", "park") for e in written["trees"])


@requires_docker
def test_a_tree_without_a_code_or_point_is_dropped_and_an_unknown_class_or_year_is_left_out():
    layer = recording(TREE_LAYER)
    base = layer["features"][0]

    def tree(code: str, geometry="keep", **properties) -> dict:
        feature = copy.deepcopy(base)
        feature["properties"].update({"tunnus": code, **properties})
        if geometry != "keep":
            feature["geometry"] = geometry
        return feature

    features = [
        tree("P-1-1", kokoluokka="200 cm", istutusvuosi=20100, paatyyppi="Täydennys", kadunnimi=None, puistonnimi="KAISANIEMI"),
        tree("P-1-2", istutusvuosi=1716),
        tree("P-1-3", istutusvuosi=1995, suku=None, laji=None, suomenknimi=None),
        tree("P-1-1", istutusvuosi=1990),
        tree("P-1-4", geometry=None),
        tree("P-1-5", geometry={"type": "Point", "coordinates": [None, 60.2]}),
        tree("P-1-6", geometry={"type": "Point", "coordinates": [13.4, 52.5]}),
        tree(None),
        tree("../x"),
        tree("K-7", istutusvuosi=1960),
    ]
    # The recorded trees beside them, so more than half the answer stays usable.
    out = {e["id"].rsplit(":", 1)[1]: e for e in run("trees-0", {**layer, "features": features + layer["features"][1:]})}
    recorded = {f"tree-{f['properties']['tunnus']}" for f in layer["features"][1:]}
    assert set(out) - recorded == {"tree-P-1-1", "tree-P-1-2", "tree-P-1-3", "tree-K-7"}
    unknown = out["tree-P-1-1"]
    assert {"trunkDiameterClass", "plantingYear", "treeSetting"}.isdisjoint(unknown), "unknown codes are left out"
    assert unknown["treeSite"]["value"] == "Kaisaniemi"
    assert "plantingYear" not in out["tree-P-1-2"]
    assert out["tree-P-1-3"]["plantingYear"] == {"type": "Property", "value": 1995}
    assert {"name", "species"}.isdisjoint(out["tree-P-1-3"])
    for entity in out.values():
        assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], entity["id"]


def windows(url: str) -> list[tuple[str, str, int]]:
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
    cql = query.get("CQL_FILTER", [""])[0]
    terms = [re.fullmatch(r"(id|db_lo)(<|>=)([0-9]+)", term) for term in cql.split(" AND ")] if cql else []
    assert all(terms), cql
    return [(m[1], m[2], int(m[3])) for m in terms]


def selects(terms: list[tuple[str, str, int]], value: int) -> bool:
    return all(value < bound if op == "<" else value >= bound for _, op, bound in terms)


def test_each_source_is_a_kartta_layer_read_over_https_with_a_timeout_and_no_credentials():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for name in NOISE + TREES:
        datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())
        assert datasource["metadata"]["name"] == f"hel-{name}"
        http = datasource["spec"]["http"]
        assert http["url"].startswith(WFS) and http["verb"] == "GET" and http["timeout"] == "60s"
        assert "auth" not in json.dumps(http).lower() and "secret" not in json.dumps(http).lower()
        pipeline = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())["spec"]
        assert pipeline["source"]["dataSourceRef"]["name"] == f"hel-{name}"
        kind = "Tree" if name in TREES else "NoiseZone"
        assert pipeline["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["period"] == ("24h" if kind == "Tree" else "168h")
        assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:helsinki:helsinki-all"
        for file in (f"helsinki-datasource-{name}.yaml", f"helsinki-pipeline-{name}.yaml", f"helsinki-pipeline-{name}-bento.yaml"):
            assert file in index, file
    rail_url = yaml.safe_load((HELSINKI / "helsinki-datasource-noise-rail.yaml").read_text())["spec"]["http"]["url"]
    assert rail_url == WFS + "".join(f"%28avoindata:{layer}%29" for layer in RAIL), "one query per layer, not a join"


def test_the_windows_read_every_record_once():
    """The band windows of each roads layer and the id windows of the register leave no gap and no overlap."""
    def url(name: str) -> str:
        return yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())["spec"]["http"]["url"]

    for period, layer in ROADS.items():
        names = [f"noise-roads-{period}-{i}" for i in range(3)]
        assert all(f"typeNames=avoindata:{layer}&" in url(name) for name in names)
        for db in range(0, 125, 5):
            assert sum(selects(windows(url(name)), db) for name in names) == 1, (period, db)
    for tree_id in [0, 1, 29999, 30000, 79999, 80000, 99999, 100000, 113786, 10**7]:
        assert sum(selects(windows(url(name)), tree_id) for name in TREES) == 1, tree_id


def test_the_pipelines_of_a_group_share_one_mapping_text():
    for group in (NOISE, TREES):
        texts = {(HELSINKI / f"helsinki-pipeline-{name}-bento.yaml").read_text() for name in group}
        assert len(texts) == 1, group
        # Only the mapping itself: the DataSource is the one read, no further call to any host.
        steps = yaml.safe_load(texts.pop())["pipeline"]["processors"]
        assert [list(step) for step in steps] == [["try"]]
        assert [list(step) for step in steps[0]["try"]] == [["mapping"], ["log"], ["unarchive"]]


def test_each_endpoint_reads_only_its_type_and_the_model_declares_it():
    for name, kind in (("noise", "NoiseZone"), ("trees", "Tree")):
        endpoint = yaml.safe_load((HELSINKI / f"helsinki-endpoint-{name}.yaml").read_text())["spec"]
        assert endpoint["audience"] == "public" and endpoint["policyRef"] == f"urn:ngsi-ld:Policy:hel.fi:helsinki:public-{name}"
        assert re.fullmatch(r"[a-z2-7]{32}", endpoint["slug"])
        policy = yaml.safe_load((HELSINKI / f"helsinki-policy-public-{name}.yaml").read_text())["spec"]
        assert policy["information"] == [{"entities": [{"type": kind}]}]
        assert not any(op.startswith(("create", "update", "delete", "append", "merge", "replace", "upsert"))
                       for op in policy["operations"] if "Subscription" not in op)
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())["spec"]
    assert {"NoiseZone", "Tree"} <= set(model["classes"])
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())["definitions"]
    linkml = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    for enum in ("NoiseSource", "NoiseAssessmentPeriod", "TrunkDiameterClass", "TreeSetting"):
        assert schema[enum]["enum"] == list(linkml["enums"][enum]["permissible_values"]), enum
