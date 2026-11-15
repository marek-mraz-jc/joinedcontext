"""Prague's IPR layers without a name field (T-5682, PL-03): the memorials and war graves and the
post offices (one PointOfInterest per place, datasets praha-pietni-mista and praha-posty) and the
technical elements of the shared green register drawn as points, lines and areas (one GreenAsset per
element, dataset praha-technicke-prvky-zelene).

Each pipeline reads its layer's description from IPR's ArcGIS server, counts the layer, then reads it
in pages of 2000 records. Here a server on this machine answers those requests from real records
recorded on 2026-10-25 (`fixtures/praha/ipr-nameless.json`: each layer's description cut to the
fields the mapping reads, and three records: two memorial records at one place, and for the green
layers one with a kind, material or surface and a district as manager, and one with neither). The
edge cases are that answer edited in the one place they name. Each entity is checked against the
space's generated JSON Schema, which the gateway enforces, and a PointOfInterest also against the
pinned Smart Data Models schema.
"""

import copy
import http.server
import json
import re
import subprocess
import threading
import time
import urllib.parse
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-nameless")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/ipr-nameless.json").read_text())
SPACE = "praha-mesto"
HOST = "https://mp.iprpraha.cz"
# pipeline: (type, id prefix, service category, geoportal item, service, cap)
LAYERS = {
    "point-war-graves": ("PointOfInterest", "point", "warMemorial", "227f70195fd04dc8b429b04ac8ce6cae", "URK_CUR_URK_VALHROBPIETMIST_B", 10000),
    "point-post-offices": ("PointOfInterest", "point", "postOffice", "431d4d6de82f4ce3b29c62cf82198671", "FSV_CUR_KV_POSTY_B", 500),
    "green-tech-points": ("GreenAsset", "green", None, "a62d904e397f4320b299f37760d818da", "OPK_CUR_OPK_SMZ_TP_B", 30000),
    "green-tech-lines": ("GreenAsset", "green", None, "d187ec184f6e4cd0b55e040df4bd717d", "OPK_CUR_OPK_SMZ_TP_L", 5000),
    "green-tech-areas": ("GreenAsset", "green", None, "6960e7e91f364502ba4325ad85e2d8f4", "OPK_CUR_OPK_SMZ_TP_P", 50000),
}
DATASETS = {"pietni-mista": ["point-war-graves"], "posty": ["point-post-offices"],
            "technicke-prvky-zelene": ["green-tech-points", "green-tech-lines", "green-tech-areas"]}


def layer_set(name: str) -> str:
    """The set a pipeline writes, which names its ids and its recording."""
    return name.split("-", 1)[1]


def server_for(layer: dict, requests: list[str], count=None):
    """IPR's FeatureServer for one layer: its count (`count`, else the records') and the records as
    the one page. Each request path is kept."""

    class Layer(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            if query.get("returnCountOnly") == ["true"]:
                body = {"count": len(layer["features"]) if count is None else count}
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
    """The pipeline `name` over its layer as `server_for` answers it: the entities written, the errors
    of the messages the runner refuses (an errored message fails the run, nothing of it is written)
    and the requests it sent."""
    layer = RECORDED[layer_set(name)] if layer is None else layer
    requests: list[str] = []
    server = server_for(layer, requests, **answers)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
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
            input=json.dumps(layer["description"]).encode(), capture_output=True, timeout=180, check=False,
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


def place(feature: dict) -> str:
    """Where a record stands, its identity: a MultiPoint of one point is the Point it is."""
    geometry = feature["geometry"]
    if geometry["type"] == "MultiPoint" and len(geometry["coordinates"]) == 1:
        return json.dumps(geometry["coordinates"][0])
    return json.dumps(geometry["coordinates"])


def by_place(name: str, entities: list[dict]) -> dict[str, dict]:
    kind, prefix = LAYERS[name][:2]
    for e in entities:
        assert re.fullmatch(rf"urn:ngsi-ld:{kind}:praha\.eu:{SPACE}:{prefix}-{layer_set(name)}-[0-9a-f]{{8}}(-[0-9a-f]{{4}}){{3}}-[0-9a-f]{{12}}", e["id"]), e["id"]
    return {json.dumps(e["location"]["value"]["coordinates"]): e for e in entities}


def edited(recording: str, edit) -> dict:
    """A copy of one recording, `edit` applied to its records."""
    layer = copy.deepcopy(RECORDED[recording])
    edit(layer["features"])
    return layer


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: outcome(name) for name in LAYERS}


@requires_docker
def test_every_place_is_one_entity_the_gateway_and_the_sdm_accept(written):
    for name, (kind, _, category, item, _, _) in LAYERS.items():
        entities, errors, _ = written[name]
        assert not errors, (name, errors)
        assert sorted(by_place(name, entities)) == sorted({place(f) for f in RECORDED[layer_set(name)]["features"]}), name
        for entity in entities:
            assert entity["type"] == kind
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            if kind == "PointOfInterest":
                assert entity["serviceCategory"]["value"] == category
                assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity), entity["id"]


@requires_docker
def test_the_records_of_one_place_are_one_entity(written):
    """The memorial register keeps 5,931 records at 1,197 points; two records at one point are one place."""
    features = RECORDED["war-graves"]["features"]
    assert place(features[0]) == place(features[1]) != place(features[2])
    entities, _, _ = written["point-war-graves"]
    assert len(entities) == 2


@requires_docker
def test_a_nameless_record_is_named_by_its_kind_and_its_district(written):
    names = {name: {k: e["name"]["languageMap"]["cs"] for k, e in by_place(name, written[name][0]).items()} for name in LAYERS}
    assert set(names["point-war-graves"].values()) == {"Pietní místo"}
    assert set(names["point-post-offices"].values()) == {"Pošta"}
    # The richest recorded element: IPR's word for it and the city district that manages it.
    rich, plain, _ = RECORDED["tech-points"]["features"]
    point = by_place("green-tech-points", written["green-tech-points"][0])[place(rich)]
    assert point["name"]["languageMap"] == {"cs": "Umělecký artefakt jiný – MČ Praha 14"}
    assert point["facilityType"]["value"] == "Umělecký artefakt jiný"
    assert point["areaServed"]["value"] == "MČ Praha 14"
    # No element code and a department as manager: the group's word alone.
    other = by_place("green-tech-points", written["green-tech-points"][0])[place(plain)]
    assert other["name"]["languageMap"]["cs"] == other["facilityType"]["value"]
    assert " – " not in other["name"]["languageMap"]["cs"]
    line = by_place("green-tech-lines", written["green-tech-lines"][0])[place(RECORDED["tech-lines"]["features"][0])]
    assert line["name"]["languageMap"] == {"cs": "Oplocení – MČ Praha-Zličín"} and line["constructionMaterial"]["value"] == "Zdivo"
    area = by_place("green-tech-areas", written["green-tech-areas"][0])[place(RECORDED["tech-areas"]["features"][0])]
    assert area["name"]["languageMap"] == {"cs": "Cesta – MČ Praha 14"}
    assert area["location"]["value"]["type"] in ("Polygon", "MultiPolygon")
    assert "surfaceType" in area and area["dateModified"]["value"] == "2026-06-30T08:16:17Z"


@requires_docker
def test_the_point_layers_read_nothing_but_the_place():
    """The memorial and post office layers publish the GUID and IPR's provider number beside the
    point; neither is written, and no attribute is invented for them."""
    entities, errors, requests = outcome("point-post-offices")
    assert not errors, errors
    for entity in entities:
        assert set(entity) == {"id", "type", "name", "serviceCategory", "location", "dataProvider", "source", "expiresAt"}
        assert entity["location"]["value"]["type"] == "Point" and len(entity["location"]["value"]["coordinates"]) == 2
    page = next(r for r in requests if "resultOffset" in r)
    assert urllib.parse.parse_qs(urllib.parse.urlsplit(page).query)["outFields"] == ["globalid"]


@requires_docker
def test_an_outside_point_a_second_guid_and_an_edit_date_out_of_range_are_counted_out():
    future = int((time.time() + 30 * 86400) * 1000)

    def edit(features):
        first, second, third = features
        third["geometry"] = {"type": "MultiPoint", "coordinates": [[17.1077, 48.1486]]}  # Bratislava
        features.append(copy.deepcopy(first))  # the same GUID again
        second["properties"]["aktualizace"] = future

    entities, errors, _ = outcome("green-tech-points", edited("tech-points", edit))
    assert not errors, errors
    features = RECORDED["tech-points"]["features"]
    written = by_place("green-tech-points", entities)
    assert sorted(written) == sorted([place(features[0]), place(features[1])])
    assert len(entities) == 2
    # An edit date after today is left out, the element is written.
    assert "dateModified" not in written[place(features[1])]


@requires_docker
def test_a_code_outside_the_layers_list_is_left_out():
    def edit(features):
        features[0]["properties"] |= {"kod_tp": 9999, "kod_sk_tp": 9999, "kod_k_mat": 9999, "kod_sprav": 9999}

    entities, errors, _ = outcome("green-tech-lines", edited("tech-lines", edit))
    assert not errors, errors
    element = by_place("green-tech-lines", entities)[place(RECORDED["tech-lines"]["features"][0])]
    # The kind falls back to the layer's own word; nothing made up stands for the unknown codes.
    assert element["name"]["languageMap"] == {"cs": "Technický prvek"} and element["facilityType"]["value"] == "Technický prvek"
    assert "constructionMaterial" not in element and "areaServed" not in element


@requires_docker
def test_a_page_where_fewer_than_half_the_records_are_usable_fails_and_writes_nothing():
    def edit(features):
        for feature in features[:2]:
            feature["geometry"] = {"type": "Point", "coordinates": [0, 0]}

    entities, errors, _ = outcome("point-post-offices", edited("post-offices", edit))
    assert entities == [] and len(errors) == 1 and "only 1 of 3 records" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("count, words", [(0, "counted no record"), (501, "more than the 500")])
def test_a_count_of_none_or_over_the_cap_fails_the_run(count, words):
    entities, errors, requests = outcome("point-post-offices", count=count)
    assert entities == [] and len(errors) == 1 and words in errors[0], errors
    assert not any("resultOffset" in r for r in requests)


@requires_docker
def test_a_description_without_a_field_list_fails_the_run():
    layer = copy.deepcopy(RECORDED["tech-areas"])
    layer["description"] = {"error": {"code": 400, "message": "Invalid URL"}}
    entities, errors, requests = outcome("green-tech-areas", layer)
    assert entities == [] and len(errors) == 1 and "did not answer its field list" in errors[0], errors
    assert requests == []


def test_the_five_mappings_differ_only_in_their_lets():
    def normal(name: str) -> str:
        text = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        body = text[text.index("pipeline:\n"):]
        return re.sub(r"(?m)^            let (set|prefix|type|category|kind|item|service|fields|order|cap) = .*\n", "", body)

    assert len({normal(name) for name in LAYERS}) == 1


def test_every_layer_is_seeded_as_a_datasource_a_pipeline_and_a_mapping_on_iprs_server():
    for name, (kind, prefix, category, item, service, cap) in LAYERS.items():
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert datasource["spec"]["http"]["url"] == f"{HOST}/arcgis/rest/services/Hosted/{service}/FeatureServer/0?f=json"
        assert "authorization" not in datasource["spec"]["http"]
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        assert pipeline["metadata"]["name"] == name
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-{name}"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in mapping and f'let service = "{service}"' in mapping
        assert f'let prefix = "{prefix}"' in mapping and f"let cap = {cap}" in mapping
        assert (f'let category = "{category}"' if category else "let category = null") in mapping
        # Every request goes to IPR's one host, and every answer is size-capped.
        assert set(re.findall(r"url: '?(https?://[^/]+)/", mapping)) == {HOST}
        assert "16777216" in mapping and "1048576" in mapping


def test_the_public_read_of_each_dataset_is_its_own_layers_only():
    sample = {name: f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:{prefix}-{layer_set(name)}-20c64a6b-6c3d-4348-a89f-77e985f95c05"
              for name, (kind, prefix, *_) in LAYERS.items()}
    patterns = {}
    for dataset, names in DATASETS.items():
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{dataset}.yaml").read_text())
        (information,) = policy["spec"]["information"]
        assert [e["type"] for e in information["entities"]] == [LAYERS[names[0]][0]]
        patterns[dataset] = re.compile(information["entities"][0]["idPattern"])
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{dataset}.yaml").read_text())
        assert endpoint["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:{SPACE}:public-{dataset}"
        assert len(endpoint["spec"]["catalog"]["source"]) == len(names)
    for name, entity in sample.items():
        assert {d for d, p in patterns.items() if p.fullmatch(entity)} == {d for d, n in DATASETS.items() if name in n}, name
    # The amenities, places and green datasets beside them read none of these.
    for other in ("vybavenost", "mista", "sluzby", "zelen", "stromy"):
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{other}.yaml").read_text())
        for selector in policy["spec"]["information"][0]["entities"]:
            assert not any(re.fullmatch(selector.get("idPattern", ".*"), e) for e in sample.values()
                           if e.split(":")[2] == selector["type"]), other
