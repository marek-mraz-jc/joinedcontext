"""Cadastral parcels of Banská Bystrica (T-4697): the kataster mapping run in Bento against a saved
page of the INSPIRE Cadastral Parcels service (four register C parcels of 801062 Banská Bystrica,
read 2026-10-19) served by a stand-in, so the entities, their attribution, the scope and the
failures that must stop a run are checked without the network."""
import copy
import http.server
import json
import shutil
import subprocess
import threading
import urllib.parse
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-banskabystrica-kataster")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/banskabystrica"
MAPPING = CITY / "pipeline-kataster-bento.yaml"
PAGE = Path(__file__).resolve().parent / "fixtures/banskabystrica/kataster-801062-page.json"
SPACE = "banskabystrica-verejne"
SERVICE = "https://inspirews.skgeodesy.sk"


class Stand(http.server.BaseHTTPRequestHandler):
    """The cadastre's items: the first page as told, every later page empty."""

    first: tuple[int, bytes] = (200, b"")
    asked: list[dict] = []

    def do_GET(self):
        query = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(self.path).query))
        Stand.asked.append(query)
        if query.get("startIndex") == "0":
            status, answer = Stand.first
        else:
            body = json.loads(PAGE.read_text())
            status, answer = 200, json.dumps({**body, "features": [], "numberReturned": 0}).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/geo+json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, *_):
        pass


def page() -> dict:
    return json.loads(PAGE.read_text())


def kataster(first: dict | None = None, status: int = 200, third: str | None = "0") -> subprocess.CompletedProcess:
    Stand.first = (status, json.dumps(page() if first is None else first).encode())
    Stand.asked = []
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stand)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        # The fetch goes to the stand-in; the `source` each parcel names stays the service's own.
        text = MAPPING.read_text().replace(f'url: {SERVICE}', f'url: http://127.0.0.1:{server.server_port}')
        if third is not None:
            text = text.replace("let third = (timestamp_unix() / 28800).floor() % $thirds", f"let third = {third}")
        config = {
            "input": {"generate": {"mapping": 'root = ""', "count": 1, "interval": ""}},
            "pipeline": yaml.safe_load(text)["pipeline"],
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "error"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "--network", "host", "-e", f"JC_SPACE={SPACE}",
             "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def written(result: subprocess.CompletedProcess) -> list[dict]:
    assert result.returncode == 0, result.stderr.decode()
    return [e for line in result.stdout.decode().splitlines() if line.strip() for e in json.loads(line)]


@pytest.fixture(scope="module")
def parcels():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in written(kataster())}


@requires_docker
def test_each_parcel_is_one_entity_the_model_takes_with_its_attribution(parcels):
    assert len(parcels) == len(page()["features"]) == 4
    for entity in parcels.values():
        assert not open_data.schema_errors(CITY / "banskabystrica-verejne.v1.schema.json", entity), entity["id"]
        assert entity["type"] == "CadastralParcel"
        assert entity["dataProvider"]["value"].startswith("Úrad geodézie, kartografie a katastra SR")
        assert "CC BY 4.0" in entity["dataProvider"]["value"]
        assert entity["location"]["value"]["type"] == "Polygon"


@requires_docker
def test_a_parcel_reads_its_number_register_area_and_cadastral_area(parcels):
    parcel = parcels[f"urn:ngsi-ld:CadastralParcel:banskabystrica.sk:{SPACE}:801062_1266-3.C"]
    assert parcel["parcelNumber"]["value"] == "1266/3"
    assert parcel["register"]["value"] == "C"
    assert parcel["cadastralAreaCode"]["value"] == "801062"
    assert parcel["cadastralAreaName"]["value"] == "Banská Bystrica"
    assert parcel["nationalCadastralReference"]["value"] == "801062_1266/3.C"
    assert parcel["areaValue"] == {"type": "Property", "value": 817, "unitCode": "MTK"}
    assert parcel["name"]["languageMap"] == {"sk": "Parcela C 1266/3, k. ú. Banská Bystrica",
                                             "en": "Parcel C 1266/3, cadastral area Banská Bystrica"}
    assert parcel["source"]["value"] == SERVICE + "/geoserver/cp/ogc/features/v1/collections/cp:CP.CadastralParcel/items/CP.1021834"


@requires_docker
def test_only_the_parcels_own_slots_are_kept_and_no_owner(parcels):
    source = {f["properties"]["nationalCadastralReference"]: f for f in page()["features"]}
    for entity in parcels.values():
        assert set(entity) <= {"id", "type", "name", "parcelNumber", "register", "cadastralAreaCode", "cadastralAreaName",
                               "nationalCadastralReference", "areaValue", "location", "dataProvider", "source"}
        assert entity["location"]["value"] == source[entity["nationalCadastralReference"]["value"]]["geometry"]
    assert "inspireId" not in json.dumps(list(parcels.values()))


@requires_docker
def test_one_area_is_asked_for_page_by_page_by_its_code_a_third_of_the_pages_a_run():
    assert len(written(kataster())) == 4
    assert {q["filter"] for q in Stand.asked} == {"nationalCadastralReference LIKE '801062_%'"}
    assert all(q["limit"] == "1000" and q["filter-lang"] == "cql-text" for q in Stand.asked)
    # The three thirds together ask every page once: a run fits the Portal's 15 min limit.
    asked = []
    for third in ("0", "1", "2"):
        kataster(third=third)
        pages = sorted(int(q["startIndex"]) for q in Stand.asked)
        assert len(pages) == 10, (third, pages)
        asked += pages
    assert sorted(asked) == [n * 1000 for n in range(30)]
    # Without a third named, the run's 8 h slot of the day picks one.
    kataster(third=None)
    assert len(Stand.asked) == 10


def test_the_three_sunday_runs_fall_in_three_slots_and_no_period_overrides_them():
    spec = yaml.safe_load((CITY / "pipeline-kataster.yaml").read_text())["spec"]
    assert "period" not in spec and spec["class"] == "scheduled"
    minute, hours, *_ = spec["schedule"].split()
    assert sorted({int(h) // 8 for h in hours.split(",")}) == [0, 1, 2]


@requires_docker
def test_a_feature_of_another_area_or_without_an_outline_is_left_out():
    answer = page()
    point, elsewhere = copy.deepcopy(answer["features"][0]), copy.deepcopy(answer["features"][1])
    point["geometry"] = {"type": "Point", "coordinates": [19.15, 48.73]}
    point["properties"]["nationalCadastralReference"] = "801062_9999/1.C"
    elsewhere["properties"]["nationalCadastralReference"] = "801101_1266/20.C"
    answer["features"] += [point, elsewhere]
    references = {e["nationalCadastralReference"]["value"] for e in written(kataster(answer))}
    assert references == {f["properties"]["nationalCadastralReference"] for f in page()["features"]}


@pytest.mark.parametrize("first, status, said", [
    ({**page(), "numberMatched": 30001}, 200, b"more than the 30 pages"),
    ({**page(), "features": [], "numberReturned": 0}, 200, b"no parcel for area 801062"),
    ({"code": "InvalidParameterValue"}, 200, b"no feature list for area 801062"),
    ({}, 503, b"did not come"),
])
@requires_docker
def test_a_short_or_failed_answer_fails_the_run(first, status, said):
    result = kataster(first, status)
    assert said in result.stderr + result.stdout
    # Nothing of the failed page reaches the writer as parcels.
    lines = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip().startswith("[")]
    assert not [e for line in lines for e in line if isinstance(e, dict) and e.get("type") == "CadastralParcel"]


def test_the_dataset_is_cc_by_with_the_cadastres_credit_and_its_own_read():
    spec = yaml.safe_load((CITY / "endpoint-kataster.yaml").read_text())["spec"]
    assert spec["contextSpaceRef"] == SPACE and spec["audience"] == "public"
    assert spec["publish"]["ckan"]["license"] == "cc-by" and spec["catalog"]["license"] == "CC_BY_4_0"
    assert "Úrad geodézie, kartografie a katastra SR" in spec["catalog"]["attribution"]["sk"]
    assert spec["catalog"]["personalData"] == "does-not-contain"
    assert "ogc-features" in spec["enabledRepresentations"]
    policy = yaml.safe_load((CITY / "policy-kataster-read.yaml").read_text())["spec"]
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["CadastralParcel"]
    for name in ("policy-verejne-read.yaml", "policy-verejne-pipelines-write.yaml"):
        rules = yaml.safe_load((CITY / name).read_text())["spec"]["information"]
        assert "CadastralParcel" in [e["type"] for rule in rules for e in rule["entities"]], name


def test_the_parcels_keep_no_history_and_the_pipeline_runs_weekly_over_the_https_service():
    space = yaml.safe_load((CITY / "space-verejne.yaml").read_text())["spec"]
    assert {"entityType": "CadastralParcel", "history": "off"} in space["retention"]
    pipeline = yaml.safe_load((CITY / "pipeline-kataster.yaml").read_text())["spec"]
    assert pipeline["output"]["type"] == "CadastralParcel"
    assert pipeline["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:banskabystrica.sk:{SPACE}:{SPACE}"
    processors = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    (fetch,) = [p["http"] for p in processors if "http" in p]
    assert fetch["url"].startswith(SERVICE + "/geoserver/cp/ogc/features/v1/")
    assert "skip_cert_verify" not in json.dumps(processors)
    model = yaml.safe_load((CITY / "datamodel-verejne.yaml").read_text())["spec"]
    assert "CadastralParcel" in model["classes"]
