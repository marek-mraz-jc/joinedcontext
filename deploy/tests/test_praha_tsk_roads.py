"""TSK hl. m. Prahy's road layers on the IPR geoportal as RoadSegment (T-5677, PL-03): the street sections
with their road class (the dataset praha-ulicni-useky) and the summer street cleaning areas with their
cleaning dates (praha-letni-udrzba).

Each pipeline reads its layer's description from IPR's ArcGIS server, counts the records its `where`
selects, then reads them in pages. Here a server on this machine answers those requests from real records
recorded on 2026-10-26 (`fixtures/praha/tsk-roads.json`: each layer's field list; one street section per
road class, one with no type given and one defunct section; the first three cleaning areas). The edge
cases are that answer edited in the one place they name. Each entity is checked against the space's
generated JSON Schema, which the gateway enforces, and Smart Data Models' RoadSegment.
"""

import copy
import http.server
import json
import re
import subprocess
import threading
import urllib.parse
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-tsk-roads")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/tsk-roads.json").read_text())
SPACE = "praha-mesto"
HOST = "https://mp.iprpraha.cz"
# set: (geoportal item, service, the record key, its bands, page size, Endpoint)
LAYERS = {
    "useky": ("1bbe08c6c9f44c70810957c5e10f22db", "DOP_CUR_DOP_TSK_ULICUSPAKOM_L", "objectid",
              ["objectid<228849", "objectid>=228849 AND objectid<238849", "objectid>=238849 AND objectid<248849",
               "objectid>=248849 AND objectid<258849", "objectid>=258849"], 2000, "ulicni-useky"),
    "letni": ("98b5292d85404383a6b5d526fa0e752c", "DOP_CUR_DOP_TSK_LU_TERMINY_P", "oid", ["oid<55745", "oid>=55745"], 500, "letni-udrzba"),
}
PIPELINES = [f"tsk-{s}-{k}" for s, layer in LAYERS.items() for k in range(len(layer[3]))]
# SDM RoadSegment requires four members TSK does not publish: no road register to point refRoad at,
# no start and end point of its own, no vehicle types (T-5677).
ACCEPTED = {f"'{m}' is a required property" for m in ("refRoad", "startPoint", "endPoint", "allowedVehicleType")}


def server_for(layer: dict, requests: list[str], count=None):
    """IPR's FeatureServer for one layer: its count (`count`, else the records') and the records from
    resultOffset, resultRecordCount at a time. Each request path is kept."""

    class Layer(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            if query.get("returnCountOnly") == ["true"]:
                body = {"count": len(layer["features"]) if count is None else count}
            else:
                start, size = int(query["resultOffset"][0]), int(query["resultRecordCount"][0])
                body = {"type": "FeatureCollection", "features": layer["features"][start:start + size]}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body).encode())

        def log_message(self, *_):
            pass

    return http.server.ThreadingHTTPServer(("127.0.0.1", 0), Layer)


def outcome(name: str, layer: dict | None = None, **answers) -> tuple[list[dict], list[str], list[str]]:
    """The pipeline `name` over its layer as `server_for` answers it: the entities written, the errors
    of the messages the runner refuses and the requests it sent."""
    layer = RECORDED[name.split("-")[1]] if layer is None else layer
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
            input=json.dumps(layer["description"]).encode(), capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()
    assert result.returncode == 0, result.stderr.decode()
    writes, errors = [], []
    for line in result.stdout.decode().splitlines():
        message = json.loads(line) if line.strip() else []
        if isinstance(message, dict) and "__error" in message:
            errors.append(message["__error"])
        else:
            writes.append(message)
    assert all(isinstance(w, list) for w in writes), writes
    return [e for w in writes for e in w], errors, requests


def edited(layer: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[layer])
    edit(document["features"])
    return document


def guid(feature: dict) -> str:
    return feature["properties"]["globalid"].strip("{}").lower()


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {layer: outcome(f"tsk-{layer}-0") for layer in LAYERS}


@requires_docker
def test_every_record_is_one_road_segment_the_gateway_accepts(written):
    for layer, (item, *_rest) in LAYERS.items():
        entities, errors, _ = written[layer]
        assert not errors, (layer, errors)
        kept = [f for f in RECORDED[layer]["features"] if f["properties"].get("typkomunik") != 25]
        by_id = {e["id"]: e for e in entities}
        assert sorted(by_id) == sorted(f"urn:ngsi-ld:RoadSegment:praha.eu:{SPACE}:tsk-{layer}-{guid(f)}" for f in kept), layer
        for feature in kept:
            entity = by_id[f"urn:ngsi-ld:RoadSegment:praha.eu:{SPACE}:tsk-{layer}-{guid(feature)}"]
            assert entity["type"] == "RoadSegment"
            assert entity["location"]["value"] == {k: feature["geometry"][k] for k in ("type", "coordinates")}
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha, TSK hl. m. Prahy (CC BY 4.0)"
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert {e.split(": ", 1)[1] for e in open_data.sdm_errors("dataModel.Transportation/RoadSegment", entity)} <= ACCEPTED, entity["id"]


@requires_docker
def test_a_street_section_carries_its_name_its_kind_of_way_and_its_road_class(written):
    entities, _, _ = written["useky"]
    by_name = {e["name"]["languageMap"]["cs"]: e for e in entities}
    assert {n: (e.get("facilityType", {}).get("value"), e.get("roadClass", {}).get("value")) for n, e in by_name.items()} == {
        "NN7534-NN7616": (None, "NATIONAL_HIGHWAY"),
        "Náchodská-NN7568": (None, "MAJOR_DISTRICT_ROAD"),
        "Nový Zlíchov-pokračování": ("Vozovka s přil.chodníkem", "MINOR_CITY_ROAD"),
        "Chmelařská-Laténská": ("Samostatný chodník pro pěší", "MINOR_CITY_ROAD"),
        "Na Šedivé-pokračování": (None, None),
    }
    # The defunct section (typkomunik 25, zaniklý úsek) is no road.
    assert "Choceradská-NN4866" not in by_name


@requires_docker
def test_a_cleaning_area_carries_its_street_and_its_dates_as_iso_days(written):
    entities, _, _ = written["letni"]
    assert {e["name"]["languageMap"]["cs"]: e["streetCleaningDates"]["value"] for e in entities} == {
        "Generála Píky": ["2026-04-07", "2026-10-05"],
        "Banskobystrická": ["2026-04-07", "2026-10-05"],
        "Buzulucká": ["2026-04-07", "2026-10-05"],
    }


@requires_docker
def test_each_layer_is_read_in_pages_of_its_size_in_record_order(written):
    for layer, (_, _, key, bands, page, _) in LAYERS.items():
        _, _, requests = written[layer]
        count, *pages = requests
        assert urllib.parse.parse_qs(urllib.parse.urlsplit(count).query) == {
            "where": [bands[0]], "returnCountOnly": ["true"], "f": ["json"]}
        (query,) = [urllib.parse.parse_qs(urllib.parse.urlsplit(p).query) for p in pages]
        assert query["where"] == [bands[0]] and query["orderByFields"] == [key] and query["resultOffset"] == ["0"]
        assert query["resultRecordCount"] == [str(page)] and query["outSR"] == ["4326"] and query["f"] == ["geojson"]


@requires_docker
def test_a_date_that_is_no_day_an_area_without_dates_a_record_outside_prague_and_a_second_copy_are_dropped():
    def edit(features):
        features[0]["properties"]["day"] = "31.02.2026, 05.10.2026"
        features[1]["properties"]["day"] = " "
        features.append(copy.deepcopy(features[2]))
        features.append(copy.deepcopy(RECORDED["letni"]["features"][0]))
        features[-1]["properties"]["globalid"] = "{00000000-0000-4000-8000-000000000000}"
        features[-1]["geometry"]["coordinates"][0][0] = [17.1077, 48.1486]  # Bratislava
        # Three more areas of their own, so the page keeps the half a page needs.
        for n in range(3):
            features.append(copy.deepcopy(features[2]))
            features[-1]["properties"]["globalid"] = f"{{00000000-0000-4000-8000-00000000000{n + 1}}}"
    entities, errors, _ = outcome("tsk-letni-0", edited("letni", edit))
    assert not errors, errors
    assert [e["name"]["languageMap"]["cs"] for e in entities] == ["Buzulucká"] * 4
    assert not any(e["id"].endswith("-00000000-0000-4000-8000-000000000000") for e in entities)


@requires_docker
def test_a_record_without_a_globalid_or_with_an_unknown_road_type_is_dropped():
    def edit(features):
        features[0]["properties"]["globalid"] = None
        features[1]["properties"]["typkomunik"] = 99
    entities, errors, _ = outcome("tsk-useky-0", edited("useky", edit))
    assert not errors, errors
    assert len(entities) == 3


@requires_docker
def test_a_page_where_fewer_than_half_survive_fails_and_writes_nothing():
    def edit(features):
        for f in features:
            f["geometry"] = {"type": "Point", "coordinates": [14.42, 50.08]}
    entities, errors, _ = outcome("tsk-useky-0", edited("useky", edit))
    assert entities == [] and len(errors) == 1 and "only 0 of 6" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("count, words", [(0, "counted no record"), (11001, "more than the 11000")])
def test_a_count_of_none_or_over_the_cap_fails_the_run(count, words):
    entities, errors, requests = outcome("tsk-useky-0", count=count)
    assert entities == [] and len(errors) == 1 and words in errors[0], errors
    assert not any("resultOffset" in r for r in requests)


@requires_docker
@pytest.mark.parametrize("description, words", [
    ({"error": {"code": 400, "message": "Invalid URL"}}, "did not answer its field list"),
    ({"fields": [{"name": "globalid"}, {"name": "nazev"}]}, "no longer lists every field of globalid,nazev,day"),
])
def test_a_description_without_the_fields_fails_the_run(description, words):
    layer = copy.deepcopy(RECORDED["letni"])
    layer["description"] = description
    entities, errors, requests = outcome("tsk-letni-0", layer)
    assert entities == [] and len(errors) == 1 and words in errors[0], errors
    assert requests == []


def test_the_pipelines_share_one_mapping_and_cover_each_record_once():
    def body(name: str) -> str:
        mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        return re.sub(r"\n            let \w+ = .*", "", mapping.split("pipeline:\n", 1)[1])

    assert len({body(n) for n in PIPELINES}) == 1
    for set_, (_, _, key, bands, _, _) in LAYERS.items():
        holds = []
        for band in bands:
            clauses = [re.fullmatch(rf"{key}(<|>=)(\d+)", c).groups() for c in band.split(" AND ")]
            holds.append(lambda v, cl=clauses: all(v < int(n) if op == "<" else v >= int(n) for op, n in cl))
        # The keys IPR held on 2026-10-26 (218849-269471, 52545-58946) and room on both sides.
        for value in range(0, 400000, 7):
            assert sum(h(value) for h in holds) == 1, (set_, value)


def test_every_pipeline_is_seeded_on_iprs_server_with_its_layer_and_capped_answers():
    for set_, (item, service, key, bands, page, _) in LAYERS.items():
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-tsk-{set_}.yaml").read_text())
        assert datasource["spec"]["http"]["url"] == f"{HOST}/arcgis/rest/services/Hosted/{service}/FeatureServer/0?f=json"
        for k, band in enumerate(bands):
            name = f"tsk-{set_}-{k}"
            pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
            assert pipeline["metadata"]["name"] == name
            assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-tsk-{set_}"
            assert pipeline["spec"]["output"] == {"type": "RoadSegment", "mode": "upsert"}
            # Every part ran in 128 MiB under docker run --memory (2026-10-26).
            assert pipeline["spec"]["quotas"]["maxMemoryMb"] == 192
            mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
            assert f'let item = "{item}"' in mapping and f'let service = "{service}"' in mapping
            assert f'let where = "{urllib.parse.quote(band)}"' in mapping and f"let page = {page}\n" in mapping
            assert set(re.findall(r"url: '?(https?://[^/]+)/", mapping)) == {HOST}
            assert "16777216" in mapping and "1048576" in mapping


def test_each_layer_is_its_own_public_dataset_and_reads_only_its_own_segments():
    other = {"useky": "letni", "letni": "useky"}
    for set_, (_, service, _, _, _, endpoint) in LAYERS.items():
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{endpoint}.yaml").read_text())
        (selector,) = policy["spec"]["information"][0]["entities"]
        assert selector["type"] == "RoadSegment"
        pattern = re.compile(selector["idPattern"])
        sample = "ce91e78f-3923-458b-b680-443b5e3830c0"
        assert pattern.match(f"urn:ngsi-ld:RoadSegment:praha.eu:{SPACE}:tsk-{set_}-{sample}")
        assert not pattern.match(f"urn:ngsi-ld:RoadSegment:praha.eu:{SPACE}:tsk-{other[set_]}-{sample}")
        assert policy["spec"]["assignee"] == {"kind": "role", "id": "public"}
        assert not {"create", "update", "delete", "upsert"} & {op.lower() for op in policy["spec"]["operations"]}
        doc = yaml.safe_load((PRAHA / f"praha-endpoint-{endpoint}.yaml").read_text())
        assert doc["metadata"]["name"] == f"praha-{endpoint}"
        assert doc["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:{SPACE}:public-{endpoint}"
        assert doc["spec"]["catalog"]["personalData"] == "does-not-contain"
        assert [s["url"].split("/Hosted/")[1].split("/")[0] for s in doc["spec"]["catalog"]["source"]] == [service]
    space = yaml.safe_load((PRAHA / "praha-space-mesto.yaml").read_text())
    assert {"entityType": "RoadSegment", "history": "off"} in space["spec"]["retention"]
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    assert {f"praha-pipeline-{n}-bento.yaml" for n in PIPELINES} <= set(index)
