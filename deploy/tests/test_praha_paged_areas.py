"""Prague's IPR area layers too large for one read (T-5683, PL-03): the computed noise maps of surface
transport for the day and the night as NoiseZone (the dataset praha-hluk), and the flood areas of the
small streams as FloodZone (the dataset praha-zaplavova-uzemi).

Each pipeline reads its layer's description from IPR's ArcGIS server, counts the records its `where`
selects, then reads them in pages. A noise band is one MultiPolygon over the city, read one band per
page and split into one entity per polygon. Here a server on this machine answers those requests from
real records recorded on 2026-10-25 (`fixtures/praha/ipr-paged-areas.json`: each layer's description
cut to the fields the mapping reads; the two loudest day bands, the loudest night band, simplified as
the pipeline asks, and the two smallest areas of each flood category). The edge cases are that answer
edited in the one place they name. Each entity is checked against the space's generated JSON Schema,
which the gateway enforces, and the extension schema of its type.
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

pytestmark = pytest.mark.xdist_group("docker-praha-paged-areas")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/ipr-paged-areas.json").read_text())
SPACE = "praha-mesto"
HOST = "https://mp.iprpraha.cz"
# pipeline: (set, type, geoportal item, service, where)
PIPELINES = {
    "geo-noise-day-0": ("noise-day", "NoiseZone", "934bce6998c84d5ca9a31c4a2d654a2f", "HM_CUR_HM_EKOLA_DEN_P", "db_lo<40"),
    "geo-noise-day-1": ("noise-day", "NoiseZone", "934bce6998c84d5ca9a31c4a2d654a2f", "HM_CUR_HM_EKOLA_DEN_P", "db_lo=40"),
    "geo-noise-day-2": ("noise-day", "NoiseZone", "934bce6998c84d5ca9a31c4a2d654a2f", "HM_CUR_HM_EKOLA_DEN_P", "db_lo>=45"),
    "geo-noise-night-0": ("noise-night", "NoiseZone", "83c81bf4e8ed47c5b38c7d1344d6b41a", "HM_CUR_HM_EKOLA_NOC_P", "db_lo<35"),
    "geo-noise-night-1": ("noise-night", "NoiseZone", "83c81bf4e8ed47c5b38c7d1344d6b41a", "HM_CUR_HM_EKOLA_NOC_P", "db_lo>=35 AND db_lo<45"),
    "geo-noise-night-2": ("noise-night", "NoiseZone", "83c81bf4e8ed47c5b38c7d1344d6b41a", "HM_CUR_HM_EKOLA_NOC_P", "db_lo>=45"),
    "geo-flood-small-streams": ("flood-small-streams", "FloodZone", "00242330738c4326af30663eb7c6d7f9", "TIV_CUR_TIV_VDS_ZAPLAVA_DVT_P", "1=1"),
}
# The pipeline each recorded layer runs through here; the others differ only in their where.
RUN = {"noise-day": "geo-noise-day-2", "noise-night": "geo-noise-night-2", "flood-small-streams": "geo-flood-small-streams"}


def server_for(layer: dict, requests: list[str], count=None):
    """IPR's FeatureServer for one layer: its count (`count`, else the records') and the records
    from resultOffset, resultRecordCount at a time. Each request path is kept."""

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
    """The pipeline `name` over its layer as `server_for` answers it: the entities written (each
    message one write), the errors of the messages the runner refuses and the requests it sent."""
    layer = RECORDED[PIPELINES[name][0]] if layer is None else layer
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


def polygons(layer: str) -> list[list]:
    """The outlines of the layer's records, one per polygon once a MultiPolygon is split."""
    out = []
    for f in RECORDED[layer]["features"]:
        g = f["geometry"]
        out += g["coordinates"] if g["type"] == "MultiPolygon" else [g["coordinates"]]
    return out


def edited(layer: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[layer])
    edit(document["features"])
    return document


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {layer: outcome(name) for layer, name in RUN.items()}


@requires_docker
def test_every_polygon_is_one_entity_the_gateway_and_the_extension_accept(written):
    for layer, name in RUN.items():
        entities, errors, _ = written[layer]
        set_, kind, item, _, _ = PIPELINES[name]
        assert not errors, (layer, errors)
        assert sorted(json.dumps(e["location"]["value"]["coordinates"]) for e in entities) == sorted(
            json.dumps(p) for p in polygons(layer)), layer
        for entity in entities:
            assert entity["type"] == kind and entity["location"]["value"]["type"] == "Polygon"
            assert re.fullmatch(rf"urn:ngsi-ld:{kind}:praha\.eu:{SPACE}:geo-{set_}-[0-9a-f]{{8}}(-[0-9a-f]{{4}}){{3}}-[0-9a-f]{{12}}", entity["id"])
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not open_data.sdm_errors(f"extensions/{kind}", entity), entity["id"]


@requires_docker
def test_a_noise_polygon_carries_its_band_in_decibels_and_its_part_of_the_day(written):
    day, _, _ = written["noise-day"]
    loudest = [e for e in day if e["noiseLevelLow"]["value"] == 90]
    assert len(loudest) == 1
    (zone,) = loudest
    assert zone["noiseLevelHigh"] == {"type": "Property", "value": 95, "unitCode": "2N"}
    assert zone["assessmentPeriod"]["value"] == "day"
    assert zone["name"]["languageMap"] == {"cs": "Hluková mapa povrchové dopravy – den: 90–95 dB"}
    night, _, _ = written["noise-night"]
    assert {e["assessmentPeriod"]["value"] for e in night} == {"night"}
    assert {e["name"]["languageMap"]["cs"] for e in night} == {"Hluková mapa povrchové dopravy – noc: 85–90 dB"}


@requires_docker
def test_a_flood_category_fills_its_own_attribute(written):
    areas, _, _ = written["flood-small-streams"]
    active = [e for e in areas if "floodZoneCategory" in e]
    q100 = [e for e in areas if "floodReturnPeriod" in e]
    assert len(active) == 2 and len(q100) == 2
    assert {e["floodZoneCategory"]["value"] for e in active} == {"active"}
    assert {e["floodReturnPeriod"]["value"] for e in q100} == {100}
    assert {e["name"]["languageMap"]["cs"] for e in active} == {"Záplavové území drobných vodních toků: aktivní zóna"}
    assert not any("floodReturnPeriod" in e for e in active)


@requires_docker
def test_each_band_is_one_page_of_one_record_simplified_by_iprs_server(written):
    _, _, requests = written["noise-day"]
    count, *pages = requests
    assert urllib.parse.parse_qs(urllib.parse.urlsplit(count).query) == {
        "where": ["db_lo>=45"], "returnCountOnly": ["true"], "f": ["json"]}
    queries = [urllib.parse.parse_qs(urllib.parse.urlsplit(p).query) for p in pages]
    assert sorted(q["resultOffset"][0] for q in queries) == ["0", "1"]
    for q in queries:
        assert q["where"] == ["db_lo>=45"] and q["resultRecordCount"] == ["1"] and q["orderByFields"] == ["objectid"]
        assert q["maxAllowableOffset"] == ["0.00002"] and q["outSR"] == ["4326"] and q["f"] == ["geojson"]
        assert q["outFields"] == ["db_lo,db_hi"]
    _, _, requests = written["flood-small-streams"]
    (page,) = [urllib.parse.parse_qs(urllib.parse.urlsplit(p).query) for p in requests[1:]]
    assert page["resultRecordCount"] == ["200"] and "maxAllowableOffset" not in page


@requires_docker
def test_a_second_flood_page_is_read_at_offset_200():
    layer = edited("flood-small-streams", lambda f: f.extend(copy.deepcopy(f[0]) for _ in range(199)))
    entities, errors, requests = outcome("geo-flood-small-streams", layer)
    assert not errors, errors
    offsets = [urllib.parse.parse_qs(urllib.parse.urlsplit(r).query).get("resultOffset") for r in requests[1:]]
    assert sorted(o[0] for o in offsets) == ["0", "200"]
    # 203 records, 200 of them one area: four entities, each written under its one id.
    assert len({e["id"] for e in entities}) == 4


@requires_docker
def test_a_polygon_outside_prague_and_a_second_copy_of_one_polygon_are_dropped():
    """The owner's plausibility rule (2026-10-22T18:10Z): a polygon outside Prague's box and a second
    copy of one polygon are dropped and counted; the band's other polygons stay."""
    def edit(features):
        band = features[0]["geometry"]["coordinates"]
        band[0][0][0] = [17.1077, 48.1486]  # Bratislava
        band.append(copy.deepcopy(band[1]))
    entities, errors, _ = outcome("geo-noise-day-2", edited("noise-day", edit))
    assert not errors, errors
    first, second = (f["geometry"]["coordinates"] for f in RECORDED["noise-day"]["features"])
    assert sorted(json.dumps(e["location"]["value"]["coordinates"]) for e in entities) == sorted(
        json.dumps(p) for p in first[1:] + [second])


@requires_docker
def test_a_band_with_a_level_out_of_range_fails_the_run():
    """A band is one page: a level outside 0-125 dB leaves none of its polygons, which is a schema
    change, not a stray record, so the run fails and the bands already written stay."""
    def edit(features):
        features[1]["properties"]["db_hi"] = 950  # a typo for 95
    entities, errors, _ = outcome("geo-noise-day-2", edited("noise-day", edit))
    assert len(errors) == 1 and "only 0 of 1 polygons" in errors[0], errors


@requires_docker
def test_an_unknown_flood_category_is_dropped():
    def edit(features):
        features[0]["properties"]["kateg"] = 7
    entities, errors, _ = outcome("geo-flood-small-streams", edited("flood-small-streams", edit))
    assert not errors, errors
    assert len(entities) == 3


@requires_docker
def test_a_page_where_fewer_than_half_survive_fails_and_writes_nothing():
    def edit(features):
        for f in features:
            f["geometry"] = {"type": "Polygon", "coordinates": [[[0, 0], [0, 1], [1, 1], [0, 0]]]}
    entities, errors, _ = outcome("geo-flood-small-streams", edited("flood-small-streams", edit))
    assert entities == [] and errors and all("only 0 of" in e for e in errors), errors


@requires_docker
@pytest.mark.parametrize("count, words", [(0, "counted no record"), (41, "more than the 12")])
def test_a_count_of_none_or_over_the_cap_fails_the_run(count, words):
    entities, errors, requests = outcome("geo-noise-day-2", count=count)
    assert entities == [] and len(errors) == 1 and words in errors[0], errors
    assert not any("resultOffset" in r for r in requests)


@requires_docker
def test_a_description_without_a_field_list_fails_the_run():
    layer = copy.deepcopy(RECORDED["noise-night"])
    layer["description"] = {"error": {"code": 400, "message": "Invalid URL"}}
    entities, errors, requests = outcome("geo-noise-night-2", layer)
    assert entities == [] and len(errors) == 1 and "did not answer its field list" in errors[0], errors
    assert requests == []


def test_the_pipelines_of_a_map_differ_only_in_their_bands_and_cover_each_band_once():
    def normal(name: str) -> str:
        mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        mapping = re.sub(r"\n            let (where|cap) = .*", "", mapping)
        return mapping.split("pipeline:\n", 1)[1]

    for prefix in ("geo-noise-day", "geo-noise-night"):
        assert len({normal(f"{prefix}-{k}") for k in range(3)}) == 1, prefix
    shared = {re.sub(r"\n            let \w+ = .*", "", normal(n)) for n in PIPELINES}
    assert len(shared) == 1
    # Each 5 dB band of 0-125 dB falls in exactly one pipeline of each map.
    holds = {
        "db_lo<40": lambda lo: lo < 40, "db_lo=40": lambda lo: lo == 40, "db_lo>=45": lambda lo: lo >= 45,
        "db_lo<35": lambda lo: lo < 35, "db_lo>=35 AND db_lo<45": lambda lo: 35 <= lo < 45,
    }
    for prefix in ("geo-noise-day", "geo-noise-night"):
        wheres = [holds[PIPELINES[f"{prefix}-{k}"][4]] for k in range(3)]
        for db_lo in range(0, 125, 5):
            assert sum(w(db_lo) for w in wheres) == 1, (prefix, db_lo)


def test_every_pipeline_is_seeded_on_iprs_server_with_its_layer_and_capped_answers():
    for name, (set_, kind, item, service, where) in PIPELINES.items():
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-geo-{set_}.yaml").read_text())
        assert datasource["spec"]["http"]["url"] == f"{HOST}/arcgis/rest/services/Hosted/{service}/FeatureServer/0?f=json"
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        assert pipeline["metadata"]["name"] == name
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-geo-{set_}"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 384
        mapping = (PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in mapping and f'let service = "{service}"' in mapping
        assert f'let where = "{urllib.parse.quote(where)}"' in mapping
        # Every request goes to IPR's one host, and every answer is size-capped.
        assert set(re.findall(r"url: '?(https?://[^/]+)/", mapping)) == {HOST}
        assert "16777216" in mapping and "1048576" in mapping


def test_the_noise_map_is_its_own_public_dataset():
    policy = yaml.safe_load((PRAHA / "praha-policy-public-hluk.yaml").read_text())
    (information,) = policy["spec"]["information"]
    assert [e["type"] for e in information["entities"]] == ["NoiseZone"]
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-hluk.yaml").read_text())
    assert endpoint["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:{SPACE}:public-hluk"
    assert [s["url"].split("/Hosted/")[1].split("/")[0] for s in endpoint["spec"]["catalog"]["source"]] == [
        "HM_CUR_HM_EKOLA_DEN_P", "HM_CUR_HM_EKOLA_NOC_P"]
    space = yaml.safe_load((PRAHA / "praha-space-mesto.yaml").read_text())
    assert {"entityType": "NoiseZone", "history": "off"} in space["spec"]["retention"]
