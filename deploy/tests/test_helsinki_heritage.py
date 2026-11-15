"""T-5670: the city's heritage and shore-route maps of kartta.hel.fi in the Helsinki space (T-5663 rows 66,
74 and 80; PL-03, DM-61).

Bento runs each committed mapping over the WFS answers the deployer recorded on 2026-10-26 (T-5668,
`fixtures/helsinki/kartta/`), and every entity it writes is checked against the space's generated JSON
Schema and its Smart Data Model. The edge cases are those answers edited in the one place they name.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import re
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-heritage")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
KARTTA = Path(__file__).resolve().parent / "fixtures/helsinki/kartta"
WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?"

# pipeline -> (DataSource, recorded layer, type written, entities the recording gives)
FEEDS = {
    # 20 outlines: 17 strongpoints and 3 positions, which carry no name and are not written.
    "fortifications": ("hel-fortifications", "Maalinnoitus_rajaukset", "HeritageArea", 17),
    # 20 areas: 9 of them the printed map's white islands, a drawing aid.
    "landscape-culture": ("hel-landscape-culture", "Maisemakulttuuri_alue", "HeritageArea", 11),
    "waterfront-sites": ("hel-waterfront-sites", "rantareitin_uusien_toimintojen_paikat_2025", "PointOfInterest", 20),
}


def recording(layer: str) -> dict:
    return json.loads((KARTTA / f"{layer}.json").read_text())


def run(pipeline: str, document: dict) -> list[dict]:
    return open_data.run_all(HELSINKI / f"helsinki-pipeline-{pipeline}-bento.yaml", json.dumps(document).encode(), "helsinki")


@pytest.fixture(scope="module")
def written():
    return {name: run(name, recording(layer)) for name, (_, layer, _, _) in FEEDS.items()}


@requires_docker
def test_every_feed_writes_its_type_once_per_record_and_the_gateway_accepts_it(written):
    for name, (_, _, kind, count) in FEEDS.items():
        entities = written[name]
        assert len(entities) == count, name
        assert len({e["id"] for e in entities}) == count, name
        for entity in entities:
            assert entity["type"] == kind
            assert re.fullmatch(rf"urn:ngsi-ld:{kind}:hel\.fi:helsinki:[A-Za-z0-9-]+", entity["id"]), entity["id"]
            assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], (entity["id"],)
            model = "extensions/HeritageArea" if kind == "HeritageArea" else "dataModel.PointOfInterest/PointOfInterest"
            assert open_data.sdm_errors(model, entity) == [], entity["id"]
            assert entity["dataProvider"]["value"] and entity["source"]["value"].startswith(WFS), entity["id"]


@requires_docker
def test_a_strongpoint_is_keyed_by_its_numeral_and_its_outlines_are_one_multipolygon():
    layer = recording("Maalinnoitus_rajaukset")
    strongpoints = [f for f in layer["features"] if f["properties"]["rajaustyyppi"] == "tukikohta"]
    first = strongpoints[0]
    second = copy.deepcopy(strongpoints[1])
    second["properties"]["tukikohtanumero"] = first["properties"]["tukikohtanumero"]
    second["properties"]["paivitetty_tietopalveluun"] = "2021-03-04"
    number = first["properties"]["tukikohtanumero"]
    out = {e["id"]: e for e in run("fortifications", {**layer, "features": [first, second]})}
    assert list(out) == [f"urn:ngsi-ld:HeritageArea:hel.fi:helsinki:fortification-{number}"]
    area = next(iter(out.values()))
    assert area["heritageCategory"]["value"] == "fortification"
    assert area["name"]["languageMap"] == {"fi": f"Tukikohta {number}", "en": f"Strongpoint {number}"}
    assert area["location"]["value"]["type"] == "MultiPolygon" and len(area["location"]["value"]["coordinates"]) == 2
    assert area["dateModified"]["value"] == "2021-03-04T00:00:00Z", "the newest outline dates the strongpoint"


@requires_docker
def test_positions_unnumbered_strongpoints_and_lines_are_not_written():
    layer = recording("Maalinnoitus_rajaukset")
    strongpoint = next(f for f in layer["features"] if f["properties"]["rajaustyyppi"] == "tukikohta")
    position = next(f for f in layer["features"] if f["properties"]["rajaustyyppi"] == "laji")
    unnumbered = copy.deepcopy(strongpoint)
    unnumbered["properties"]["tukikohtanumero"] = None
    line = copy.deepcopy(strongpoint)
    line["geometry"] = {"type": "LineString", "coordinates": [[24.9, 60.2], [24.91, 60.21]]}
    assert run("fortifications", {**layer, "features": [position, unnumbered, line]}) == []


@requires_docker
def test_a_landscape_area_takes_its_kind_and_the_drawing_aids_are_dropped(written):
    kinds = {e["heritageCategory"]["value"] for e in written["landscape-culture"]}
    assert kinds == {"residentialEnsemble", "cemetery", "villaPark", "courtyard"}
    layer = recording("Maisemakulttuuri_alue")
    area = next(f for f in layer["features"] if f["properties"]["kohdetyyppi"] == "Hautausmaat")
    out = run("landscape-culture", {**layer, "features": [area]})
    assert [e["id"] for e in out] == [f"urn:ngsi-ld:HeritageArea:hel.fi:helsinki:landscape-{area['properties']['id']}"]
    assert out[0]["name"]["languageMap"] == {"fi": "Hautausmaa", "en": "Cemetery"}
    assert out[0]["location"]["value"] == area["geometry"]
    assert out[0]["dateModified"]["value"] == "2019-09-25T00:00:00Z"
    aids = []
    for kind in ("Valkoinen_saaret", "ALUEJAKO_Kaupunginosa", "KSV_tunnus", "Merinäkymä_symbolinuolet", "a kind added later"):
        aid = copy.deepcopy(area)
        aid["properties"]["kohdetyyppi"] = kind
        aids.append(aid)
    assert run("landscape-culture", {**layer, "features": aids}) == []


@requires_docker
def test_a_shore_site_is_named_by_its_first_sentence_and_described_by_every_line_the_layer_has(written):
    layer = recording("rantareitin_uusien_toimintojen_paikat_2025")
    site = copy.deepcopy(layer["features"][0])
    site["properties"].update({
        "teksti": "Sauna.  Paikalle saa rakentaa\nenintään 30 m2 saunan.", "pysakointi": "kadun varrella",
        "kaavoitustilanne": "", "kayton_rajoitukset": "None", "kaytettavyys": None,
    })
    long = copy.deepcopy(site)
    long["properties"]["id"] = 999
    long["properties"]["teksti"] = "Rajasaareen johtavalla kannaksella sijaitseva Humalluodonpuisto soveltuisi hyvin kahvilalle"
    empty = copy.deepcopy(site)
    empty["properties"]["id"] = 998
    empty["properties"]["teksti"] = "None"
    out = {e["id"].rsplit("-", 1)[1]: e for e in run("waterfront-sites", {**layer, "features": [site, long, empty]})}
    assert set(out) == {str(site["properties"]["id"]), "999"}, "a site without text is not written"
    first = out[str(site["properties"]["id"])]
    assert first["name"]["languageMap"] == {"fi": "Sauna"}
    assert first["serviceCategory"]["value"] == "waterfrontSite"
    assert first["description"]["languageMap"]["fi"] == (
        "Sauna. Paikalle saa rakentaa enintään 30 m2 saunan.\nPysäköinti: kadun varrella")
    name = out["999"]["name"]["languageMap"]["fi"]
    assert name == "Rajasaareen johtavalla kannaksella sijaitseva Humalluodonpuisto soveltuisi…"
    assert len(name) <= 81
    assert all(e["name"]["languageMap"]["fi"] for e in written["waterfront-sites"])


def test_each_source_is_one_kartta_layer_read_weekly_over_https_with_a_timeout():
    for name, (source, layer, kind, _) in FEEDS.items():
        datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())
        assert datasource["metadata"]["name"] == source
        http = datasource["spec"]["http"]
        assert http["url"] == (f"{WFS}service=WFS&version=2.0.0&request=GetFeature&outputFormat=application/json"
                               f"&srsName=EPSG:4326&typeNames=avoindata:{layer}")
        assert http["verb"] == "GET" and http["timeout"] in ("30s", "60s") and "auth" not in json.dumps(http).lower()
        pipeline = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())["spec"]
        assert pipeline["source"]["dataSourceRef"]["name"] == source
        assert pipeline["output"] == {"type": kind, "mode": "upsert"} and pipeline["period"] == "168h"
        # Only the mapping itself: no further call to any host.
        assert "http" not in yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}-bento.yaml").read_text())["pipeline"]["processors"][0]
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for name in FEEDS:
        for file in (f"helsinki-datasource-{name}.yaml", f"helsinki-pipeline-{name}.yaml", f"helsinki-pipeline-{name}-bento.yaml"):
            assert file in index, file


def test_the_heritage_endpoint_reads_only_heritage_areas_and_the_model_declares_them():
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-heritage.yaml").read_text())["spec"]
    assert endpoint["audience"] == "public" and endpoint["policyRef"] == "urn:ngsi-ld:Policy:hel.fi:helsinki:public-heritage"
    assert re.fullmatch(r"[a-z2-7]{32}", endpoint["slug"])
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-heritage.yaml").read_text())["spec"]
    assert policy["information"] == [{"entities": [{"type": "HeritageArea"}]}]
    assert not any(op.startswith(("create", "update", "delete", "append", "merge", "replace", "upsert"))
                   for op in policy["operations"] if "Subscription" not in op)
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())["spec"]
    assert "HeritageArea" in model["classes"]
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())["definitions"]
    linkml = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    assert schema["HeritageCategory"]["enum"] == list(linkml["enums"]["HeritageCategory"]["permissible_values"])
    assert schema["ServiceCategory"]["enum"] == list(linkml["enums"]["ServiceCategory"]["permissible_values"])
