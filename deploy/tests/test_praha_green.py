"""Prague's parks, green areas and trees from the IPR Praha geoportal (T-5425, PL-03): the parks and
nature parks, the basic green areas, trees and tree rows of the shared green register and the tree-row
layer, one Garden or Tree each, published as the datasets praha-zelen (Garden) and praha-stromy (Tree).
The register's areas in care and vegetation areas (T-5683) are read a tenth per pipeline, as the trees are.

Each pipeline reads its layer's description from IPR's ArcGIS server, counts the layer, then reads it
in pages of 2000 records. Here a server on this machine answers those three requests from real
records recorded on 2026-10-24 (`fixtures/praha/ipr-green.json`: each layer's description cut to the
fields the mapping reads, and its first three records). The edge cases are that answer edited in the
one place they name. Each entity is checked against the space's generated JSON Schema, which the
gateway enforces, and a Garden also against the pinned Smart Data Models Garden schema.
"""

import copy
import http.server
import json
import re
import subprocess
import threading
import time
import urllib.parse
from datetime import datetime, timedelta, timezone
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-green")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/ipr-green.json").read_text())
SPACE = "praha-mesto"
HOST = "https://mp.iprpraha.cz"
# set: (type, geoportal item, service, the where of its query)
LAYERS = {
    # The trees are read a tenth per pipeline (green-trees-0 to -9, objectid modulo 10); the first
    # stands for the ten here, test_the_ten_tree_pipelines_differ_only_in_their_tenth checks the rest.
    "trees-0": ("Tree", "802a43db41474d4db5789ef5a47f2985", "OPK_CUR_OPK_SMZ_VP_B", "kod_vp=71 AND MOD(objectid,10)=0"),
    # The areas in care and the vegetation areas, a tenth per pipeline as well (T-5683).
    "management-0": ("Garden", "77d575ed2fd14f4987474a2f26caa460", "OPK_CUR_OPK_SMZ_SPRAVA_P", "MOD(objectid,10)=0"),
    "vegetation-areas-0": ("Garden", "518e85700665441f86ded81dd0653a71", "OPK_CUR_OPK_SMZ_VP_P", "MOD(objectid,10)=0"),
    "register-tree-rows": ("Tree", "8818bda5e5e34d96af02126d9554b385", "OPK_CUR_OPK_SMZ_VP_L", "kod_vp=81"),
    "tree-rows": ("Tree", "3615fd648f1f4182b5f42dcdfba03b54", "URK_CUR_URK_SS_STROMORADI_L", "1=1"),
    "green-areas": ("Garden", "58d3857a49fc43cab69c5aad0162d024", "OPK_CUR_OPK_SMZ_ZAKLADNIPL_P", "1=1"),
    "parks": ("Garden", "bf72734397ba46b9b817ee997d38284b", "URK_CUR_URK_PARKY_P", "1=1"),
    "nature-parks": ("Garden", "48719ca373c947f99d3fbdcf341c07f2", "OPK_CUR_OPK_PRIRODNIPARKY_P", "1=1"),
}


def layer_set(name: str) -> str:
    """The set a pipeline writes, which names its ids and its recording."""
    return re.sub(r"-\d$", "", name)


def server_for(layer: dict, requests: list[str], count=None, second_page=None):
    """IPR's FeatureServer for one layer: its count (`count`, else the records'),
    the records as the first page and `second_page` at offset 2000. Each request path is kept."""

    class Layer(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            url = urllib.parse.urlsplit(self.path)
            query = urllib.parse.parse_qs(url.query)
            if query.get("returnCountOnly") == ["true"]:
                body = {"count": len(layer["features"]) if count is None else count}
            elif query.get("resultOffset") == ["2000"]:
                body = {"type": "FeatureCollection", "features": second_page or []}
            else:
                body = {"type": "FeatureCollection", "features": layer["features"]}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body).encode())

        def log_message(self, *_):
            pass

    return http.server.ThreadingHTTPServer(("127.0.0.1", 0), Layer)


def outcome(name: str, layer: dict | None = None, **answers) -> tuple[list[dict], list[str], list[str]]:
    """The pipeline `green-<name>` over its layer as `server_for` answers it: the entities written,
    the errors of the messages the runner refuses (an errored message fails the run, nothing of it is
    written) and the requests it sent."""
    layer = RECORDED[layer_set(name)] if layer is None else layer
    requests: list[str] = []
    server = server_for(layer, requests, **answers)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (PRAHA / f"praha-pipeline-green-{name}-bento.yaml").read_text()
        pipeline = yaml.safe_load(text.replace(HOST, f"http://127.0.0.1:{server.server_port}"))["pipeline"]
        pipeline["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": pipeline,
            "rate_limit_resources": yaml.safe_load(open_data.RESOURCES.read_text())["rate_limit_resources"],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "error"},
        }
        result = subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=praha.eu",
             "-e", f"JC_SPACE={SPACE}", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            # The DataSource's own read: the layer's description.
            input=json.dumps(layer["description"]).encode(), capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()
    assert result.returncode == 0, result.stderr.decode()
    entities, errors = [], []
    for line in result.stdout.decode().splitlines():
        message = json.loads(line) if line.strip() else []
        if isinstance(message, dict) and "__error" in message:
            errors.append(message["__error"])
        else:
            entities.extend(message if isinstance(message, list) else [message])
    return entities, errors, requests


def guid(feature: dict) -> str:
    """Where a record stands, its identity: a MultiPoint of one point is the Point it is."""
    geometry = feature["geometry"]
    if geometry["type"] == "MultiPoint" and len(geometry["coordinates"]) == 1:
        return json.dumps(geometry["coordinates"][0])
    return json.dumps(geometry["coordinates"])


def by_guid(name: str, entities: list[dict]) -> dict[str, dict]:
    """The entities by where they stand; each id is green-<set>-<the geometry's hash>."""
    for e in entities:
        assert re.fullmatch(rf"urn:ngsi-ld:\w+:praha\.eu:{SPACE}:green-{layer_set(name)}-[0-9a-f]{{8}}(-[0-9a-f]{{4}}){{3}}-[0-9a-f]{{12}}", e["id"]), e["id"]
    return {json.dumps(e["location"]["value"]["coordinates"]): e for e in entities}


def edited(name: str, edit) -> dict:
    layer = copy.deepcopy(RECORDED[layer_set(name)])
    edit(layer["features"])
    return layer


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: outcome(name) for name in LAYERS}


@requires_docker
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    for name, (kind, item, _, _) in LAYERS.items():
        entities, errors, _ = written[name]
        assert not errors, (name, errors)
        assert sorted(by_guid(name, entities)) == sorted(guid(f) for f in RECORDED[layer_set(name)]["features"]), name
        for entity in entities:
            assert entity["type"] == kind
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:green-{layer_set(name)}-")
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            # A geometry-hash id never comes back once redrawn: it expires four weekly runs on (F-BNT3-019).
            left = datetime.fromisoformat(entity["expiresAt"]) - datetime.now(timezone.utc)
            assert timedelta(days=27) < left <= timedelta(days=28), entity["id"]
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            if kind == "Garden":
                assert not open_data.sdm_errors("dataModel.ParksAndGardens/Garden", entity), entity["id"]


@requires_docker
def test_each_layer_is_counted_then_read_in_pages_of_its_own_query(written):
    for name, (_, _, service, where) in LAYERS.items():
        _, _, requests = written[name]
        # The description is the DataSource's own read; the mapping sends the count, then the page.
        count, page = requests
        assert count.startswith(f"/arcgis/rest/services/Hosted/{service}/FeatureServer/0/query?")
        assert urllib.parse.parse_qs(urllib.parse.urlsplit(count).query) == {
            "where": [where], "returnCountOnly": ["true"], "f": ["json"]}
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(page).query)
        # Oldest edit first where the layer dates its records, so the newest version is written last.
        dated = "aktualizace" in query["outFields"][0]
        assert query["orderByFields"] == ["aktualizace,objectid" if dated else "objectid"], name
        assert query["where"] == [where] and query["resultOffset"] == ["0"] and query["resultRecordCount"] == ["2000"]
        assert query["outSR"] == ["4326"] and query["f"] == ["geojson"]


@requires_docker
def test_the_coded_values_are_written_in_iprs_own_words(written):
    tree = by_guid("trees-0", written["trees-0"][0])[guid(RECORDED["trees"]["features"][0])]
    assert tree["vegetationType"]["value"] == "Strom"
    # A MultiPoint of one point is written as the Point it is.
    assert tree["location"]["value"]["type"] == "Point" and len(tree["location"]["value"]["coordinates"]) == 2
    assert tree["dateModified"]["value"] == "2025-02-10T00:00:00Z"
    row = by_guid("register-tree-rows", written["register-tree-rows"][0])[guid(RECORDED["register-tree-rows"]["features"][0])]
    assert row["vegetationType"]["value"] == "Stromořadí"
    assert row["careRegime"]["value"] == "Intenzitní třída údržby I" and row["areaServed"]["value"] == "MHMP-OCP"
    park = by_guid("parks", written["parks"][0])[guid(RECORDED["parks"]["features"][0])]
    assert park["name"]["languageMap"] == {"cs": "Park Belvedér"}
    assert park["greenspaceType"]["value"] == "lesopark"
    nature = by_guid("nature-parks", written["nature-parks"][0])[guid(RECORDED["nature-parks"]["features"][0])]
    assert nature["greenspaceType"]["value"] == "přírodní park" and nature["name"]["languageMap"] == {"cs": "Draháň - Troja"}
    area = by_guid("green-areas", written["green-areas"][0])[guid(RECORDED["green-areas"]["features"][0])]
    assert area["areaServed"]["value"] == "MČ Praha 9"
    managed = by_guid("management-0", written["management-0"][0])[guid(RECORDED["management"]["features"][0])]
    assert managed["areaServed"]["value"] == "MČ Praha-Klánovice" and managed["greenspaceType"]["value"] == "plocha správy zeleně"
    bed = by_guid("vegetation-areas-0", written["vegetation-areas-0"][0])[guid(RECORDED["vegetation-areas"]["features"][0])]
    assert bed["greenspaceType"]["value"] == "Záhon trvalek" and bed["areaServed"]["value"] == "MHMP-OCP"
    assert bed["careRegime"]["value"] == "Intenzitní třída údržby I"
    # The tree-row layer names nothing but the row itself.
    rows = written["tree-rows"][0]
    assert {e["vegetationType"]["value"] for e in rows} == {"Stromořadí"}


@requires_docker
def test_a_tree_outside_prague_a_second_record_of_one_guid_and_an_edit_date_in_the_future_are_counted_out():
    future = int((time.time() + 30 * 86400) * 1000)

    def edit(features):
        first, second, third = features
        third["geometry"] = {"type": "MultiPoint", "coordinates": [[17.1077, 48.1486]]}  # Bratislava
        features.append(copy.deepcopy(first))  # the same GUID again
        second["properties"]["aktualizace"] = future

    entities, errors, _ = outcome("trees-0", edited("trees-0", edit))
    assert not errors, errors
    features = RECORDED["trees"]["features"]
    written = by_guid("trees-0", entities)
    assert sorted(written) == sorted([guid(features[0]), guid(features[1])])
    assert len(entities) == 2
    # An edit date after today is left out, the tree is written.
    assert "dateModified" not in written[guid(features[1])]


@requires_docker
@requires_docker
def test_the_saved_versions_of_one_area_are_one_entity_with_the_newest_edit():
    """The register keeps each saved version of an area as a record of its own GUID with the same
    geometry; in one page or across two they are one entity, the newest edit written last."""
    first, *_ = RECORDED["green-areas"]["features"]
    older = copy.deepcopy(first)
    older["properties"] |= {"globalid": "{00000000-0000-4000-8000-000000000003}", "aktualizace": 1700000000000}
    newer = copy.deepcopy(first)
    newer["properties"] |= {"globalid": "{00000000-0000-4000-8000-000000000004}", "aktualizace": 1790000000000}
    entities, errors, _ = outcome("green-areas", edited("green-areas", lambda f: f.extend([older, newer])))
    assert not errors, errors
    assert len(entities) == 3
    assert by_guid("green-areas", entities)[guid(first)]["dateModified"]["value"] == "2026-09-21T14:13:20Z"
    entities, errors, _ = outcome("green-areas", count=2001, second_page=[older])
    assert not errors, errors
    assert {e["id"] for e in entities if json.dumps(e["location"]["value"]["coordinates"]) == guid(first)} == {
        by_guid("green-areas", entities)[guid(first)]["id"]}


@requires_docker
@pytest.mark.parametrize("name, kind", [("vegetation-areas-0", "Garden"), ("trees-0", "Tree")])
def test_a_woody_category_is_written_only_on_a_tree_since_the_view_holds_it_for_no_garden(name, kind):
    """IPR fills kod_drevin on some vegetation areas too (T-5737: dev refused every vegetation-areas
    run for the treeCategory a Garden does not hold). Each entity carries only its class's attributes."""
    def edit(features):
        features[0]["properties"]["kod_drevin"] = 2

    entities, errors, _ = outcome(name, edited(name, edit))
    assert not errors, errors
    first = by_guid(name, entities)[guid(RECORDED[layer_set(name)]["features"][0])]
    schema = json.loads((PRAHA / "praha.v1.schema.json").read_text())["definitions"][kind]["properties"]
    allowed = set(schema) | open_data.SYSTEM_MEMBERS
    for entity in entities:
        assert set(entity) <= allowed, (entity["id"], set(entity) - allowed)
    assert ("treeCategory" in first) == (kind == "Tree")
    if kind == "Tree":
        assert first["treeCategory"]["value"] == "Listnatý"


@pytest.mark.parametrize("set_, kind, where", [
    ("trees", "Tree", "kod_vp=71 AND MOD(objectid,10)={k}"),
    ("management", "Garden", "MOD(objectid,10)={k}"),
    ("vegetation-areas", "Garden", "MOD(objectid,10)={k}"),
])
def test_the_ten_pipelines_of_a_layer_differ_only_in_their_tenth(set_, kind, where):
    def normal(k: int) -> str:
        let = f'let where = "{urllib.parse.quote(where.format(k=k))}"'
        mapping = (PRAHA / f"praha-pipeline-green-{set_}-{k}-bento.yaml").read_text()
        assert mapping.count(let) == 1, k
        return mapping.replace(let, "").replace(f"green-{set_}-{k} pipeline", "").replace(f"objectid ends in {k}", "")

    assert all(normal(k) == normal(0) for k in range(10))
    for k in range(10):
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-green-{set_}-{k}.yaml").read_text())
        assert pipeline["metadata"]["name"] == f"green-{set_}-{k}"
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-green-{set_}"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
    assert not (PRAHA / f"praha-pipeline-green-{set_}.yaml").exists()


def test_a_record_without_a_guid_or_a_geometry_is_dropped():
    def edit(features):
        features[1]["properties"]["globalid"] = None
        features.append(copy.deepcopy(features[0]) | {"geometry": None})
        features[-1]["properties"] = features[-1]["properties"] | {"globalid": "{00000000-0000-4000-8000-000000000001}"}

    entities, errors, _ = outcome("green-areas", edited("green-areas", edit))
    assert not errors, errors
    features = RECORDED["green-areas"]["features"]
    assert sorted(by_guid("green-areas", entities)) == sorted([guid(features[0]), guid(features[2])])


@requires_docker
def test_a_page_where_fewer_than_half_the_records_are_usable_fails_and_writes_nothing():
    def edit(features):
        for feature in features[:2]:
            feature["geometry"] = {"type": "Polygon", "coordinates": [[[0, 0], [0, 1], [1, 1], [0, 0]]]}

    entities, errors, _ = outcome("parks", edited("parks", edit))
    assert entities == [] and len(errors) == 1 and "only 1 of 3 records" in errors[0], errors


@requires_docker
def test_a_second_page_is_read_at_offset_2000():
    features = RECORDED["tree-rows"]["features"]
    entities, errors, requests = outcome("tree-rows", count=2001, second_page=features[:1] + [
        copy.deepcopy(features[1]) | {"properties": {"globalid": "{00000000-0000-4000-8000-000000000002}"}}])
    assert not errors, errors
    pages = [urllib.parse.parse_qs(urllib.parse.urlsplit(r).query) for r in requests if "resultOffset" in r]
    assert sorted(p["resultOffset"][0] for p in pages) == ["0", "2000"]
    assert len(entities) == 5
    # A record a later page repeats, or another version of it under a new GUID, is written again
    # under its one id: an upsert, not a second tree row.
    assert len({e["id"] for e in entities}) == 3


@requires_docker
@pytest.mark.parametrize("count, words", [(0, "counted no record"), (30001, "more than the 30000")])
def test_a_count_of_none_or_over_the_cap_fails_the_run(count, words):
    entities, errors, requests = outcome("trees-0", count=count)
    assert entities == [] and len(errors) == 1 and words in errors[0], errors
    assert not any("resultOffset" in r for r in requests)


@requires_docker
def test_a_description_without_a_field_list_fails_the_run():
    layer = copy.deepcopy(RECORDED["nature-parks"])
    layer["description"] = {"error": {"code": 400, "message": "Invalid URL"}}
    entities, errors, requests = outcome("nature-parks", layer)
    assert entities == [] and len(errors) == 1 and "did not answer its field list" in errors[0], errors
    assert requests == []


def test_every_layer_is_seeded_as_a_datasource_a_pipeline_and_a_mapping_on_iprs_server():
    for name, (kind, item, service, where) in LAYERS.items():
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-green-{layer_set(name)}.yaml").read_text())
        assert datasource["spec"]["http"]["url"] == f"{HOST}/arcgis/rest/services/Hosted/{service}/FeatureServer/0?f=json"
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-green-{name}.yaml").read_text())
        assert pipeline["metadata"]["name"] == f"green-{name}"
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-green-{layer_set(name)}"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        mapping = (PRAHA / f"praha-pipeline-green-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in mapping and f'let service = "{service}"' in mapping
        assert f'let where = "{urllib.parse.quote(where)}"' in mapping
        # Every request goes to IPR's one host, and every answer is size-capped.
        assert set(re.findall(r"url: '?(https?://[^/]+)/", mapping)) == {HOST}
        assert "16777216" in mapping and "1048576" in mapping


def test_the_public_read_of_each_dataset_is_one_type_narrowed_to_the_green_ids():
    patterns = {}
    for name, kind in (("zelen", "Garden"), ("stromy", "Tree")):
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{name}.yaml").read_text())
        (information,) = policy["spec"]["information"]
        assert [e["type"] for e in information["entities"]] == [kind]
        patterns[kind] = re.compile(information["entities"][0]["idPattern"])
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{name}.yaml").read_text())
        assert endpoint["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:{SPACE}:public-{name}"
    assert patterns["Tree"].match(f"urn:ngsi-ld:Tree:praha.eu:{SPACE}:green-trees-20c64a6b-6c3d-4348-a89f-77e985f95c05")
    assert not patterns["Garden"].match(f"urn:ngsi-ld:Garden:praha.eu:{SPACE}:golemio-gardens-1")
