"""The cycle counters of Prague from Golemio (T-5681, PL-03).

Bento runs the committed mapping over seven real counters recorded from
https://api.golemio.cz/v2/bicyclecounters on 2026-10-25 at 15:25Z (`fixtures/praha/golemio-bicycle-counters.json`)
and their counts of the hour before, from /v2/bicyclecounters/detections?from=2026-10-25T14:25:00Z
(`fixtures/praha/golemio-bicycle-detections.json`), served by a stand-in that records what the
mapping asked and with which key. Among the seven: Anděl and Atletická counting both ways, the
three directions of Vítkov (one silent), Dukelských hrdinů reporting only empty counts, Dubeč with
no count at all and the ecoCounter at Podolí silent since July. The edge cases are those answers
edited in the one place they name. Each entity is checked against the space's generated JSON
Schema, which the gateway enforces.
"""

import copy
import http.server
import json
import shutil
import subprocess
import threading
import urllib.parse
from pathlib import Path
from typing import ClassVar

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-bicycle-counters")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
MAPPING = PRAHA / "praha-pipeline-bicycle-counters-bento.yaml"
FIXTURES = Path(__file__).resolve().parent / "fixtures/praha"
COUNTERS = json.loads((FIXTURES / "golemio-bicycle-counters.json").read_text())
DETECTIONS = json.loads((FIXTURES / "golemio-bicycle-detections.json").read_text())
SERVICE = "https://api.golemio.cz"
SPACE = "praha-mesto"
NOW = "2026-10-25T15:25:00Z"
KEY = "stand-in-key"


class Stand(http.server.BaseHTTPRequestHandler):
    """Golemio's detections route: the answer as told, and every request it saw."""

    answer: ClassVar[tuple[int, bytes]] = (200, b"[]")
    asked: ClassVar[list[tuple[str, dict, str | None]]] = []

    def do_GET(self):
        split = urllib.parse.urlsplit(self.path)
        Stand.asked.append((split.path, dict(urllib.parse.parse_qsl(split.query)), self.headers.get("X-Access-Token")))
        status, body = Stand.answer
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def outcome(counters: dict = COUNTERS, detections=DETECTIONS, status: int = 200, now: str = NOW) -> tuple[list[dict], list[str]]:
    """The mapping over one counter list and one detections answer, with now() pinned: the
    entities written and the error of a message the runner refuses (an errored message fails
    the run, nothing is written)."""
    Stand.answer = (status, json.dumps(detections).encode())
    Stand.asked = []
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stand)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = MAPPING.read_text().replace(f"url: {SERVICE}", f"url: http://127.0.0.1:{server.server_port}")
        text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
        pipeline = yaml.safe_load(text)["pipeline"]
        pipeline["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": pipeline,
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "all-bytes"}},
            "logger": {"level": "error"},
        }
        result = subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=praha.eu", "-e", f"JC_SPACE={SPACE}",
             "-e", f"PRAHA_GOLEMIO_TOKEN={KEY}", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=json.dumps(counters).encode(), capture_output=True, timeout=120, check=False,
        )
    finally:
        server.shutdown()
    assert result.returncode == 0, result.stderr.decode()
    answer = json.loads(result.stdout)
    if isinstance(answer, dict) and "__error" in answer:
        return [], [answer["__error"]]
    return answer, []


def run(**kwargs) -> list[dict]:
    entities, errors = outcome(**kwargs)
    assert not errors, errors
    return entities


def by_key(entities: list[dict]) -> dict[str, dict]:
    return {e["id"].rsplit(":", 1)[1]: e for e in entities}


def counters_edited(edit) -> dict:
    document = copy.deepcopy(COUNTERS)
    edit(document["features"])
    return document


def detections_edited(edit) -> list:
    rows = copy.deepcopy(DETECTIONS)
    edit(rows)
    return rows


def counter(features: list, counter_id: str) -> dict:
    return next(f for f in features if f["properties"]["id"] == counter_id)


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    entities = run()
    return by_key(entities), list(Stand.asked)


@requires_docker
def test_every_counted_direction_is_one_traffic_flow_the_gateway_accepts(written):
    entities, _ = written
    # Seven directions counted bicycles in the hour; Dukelských hrdinů's empty counts, Dubeč,
    # Vítkov's third direction and the silent ecoCounter write nothing.
    assert sorted(entities) == [
        "golemio-bicycle-camea-bc-al-pl", "golemio-bicycle-camea-bc-al-st", "golemio-bicycle-camea-bc-at-la",
        "golemio-bicycle-camea-bc-at-st", "golemio-bicycle-camea-bc-vk-hr", "golemio-bicycle-camea-bc-vk-up",
        "golemio-bicycle-camea-bc-vs-ce",
    ]
    for entity in entities.values():
        assert entity["id"].startswith("urn:ngsi-ld:TrafficFlowObserved:praha.eu:praha-mesto:golemio-bicycle-")
        assert open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity) == [], entity["id"]


@requires_docker
def test_a_direction_carries_its_newest_five_minutes_its_counter_and_the_attribution(written):
    entities, _ = written
    newest = max((d for d in DETECTIONS if d["id"] == "camea-BC_AL-PL"), key=lambda d: d["measured_to"])
    andel = entities["golemio-bicycle-camea-bc-al-pl"]
    assert andel["name"]["languageMap"] == {"cs": "Anděl (Plzeňská): Plzeňská (z centra)"}
    assert andel["location"]["value"] == {"type": "Point", "coordinates": [14.3986383, 50.0718897]}
    assert andel["vehicleType"]["value"] == "bicycle"
    assert andel["intensity"]["value"] == newest["value"]
    assert andel["intensity"]["observedAt"] == andel["dateObservedTo"]["value"] == newest["measured_to"].replace(".000Z", "Z")
    assert andel["dateObservedFrom"]["value"] == newest["measured_from"].replace(".000Z", "Z")
    assert andel["dataProvider"]["value"] == "Golemio, Operátor ICT, a.s."
    assert andel["source"]["value"] == "https://api.golemio.cz/v2/bicyclecounters"


@requires_docker
def test_the_counts_are_asked_for_the_last_hour_with_the_pipelines_own_key(written):
    _, asked = written
    assert asked == [("/v2/bicyclecounters/detections", {"from": "2026-10-25T14:25:00Z", "limit": "10000"}, KEY)]


@requires_docker
def test_out_of_area_zero_and_nameless_counters_are_dropped():
    def edit(features):
        counter(features, "camea-BC_AL-STPL")["geometry"]["coordinates"] = [16.6, 49.2]
        counter(features, "camea-BC_AT-STLA")["geometry"]["coordinates"] = [0, 0]
        counter(features, "camea-BC_VS-CE")["properties"]["directions"][0]["name"] = "  "
    entities = by_key(run(counters=counters_edited(edit)))
    assert sorted(entities) == ["golemio-bicycle-camea-bc-vk-hr", "golemio-bicycle-camea-bc-vk-up"]


@requires_docker
def test_a_direction_listed_twice_is_written_once():
    def edit(features):
        features.append(copy.deepcopy(counter(features, "camea-BC_VS-CE")))
    entities = run(counters=counters_edited(edit))
    assert [e["id"] for e in entities].count("urn:ngsi-ld:TrafficFlowObserved:praha.eu:praha-mesto:golemio-bicycle-camea-bc-vs-ce") == 1


@requires_docker
def test_an_out_of_range_count_is_left_out_and_the_direction_keeps_its_newest_plausible_one():
    rows = sorted((d for d in DETECTIONS if d["id"] == "camea-BC_VS-CE"), key=lambda d: d["measured_to"])
    newest, before = rows[-1], rows[-2]

    def edit(detections):
        for d in detections:
            if d["id"] == "camea-BC_VS-CE" and d["measured_to"] == newest["measured_to"]:
                d["value"] = 5000
            if d["id"] == "camea-BC_AL-PL":
                d["value"] = -1
    entities = by_key(run(detections=detections_edited(edit)))
    assert "golemio-bicycle-camea-bc-al-pl" not in entities
    kosire = entities["golemio-bicycle-camea-bc-vs-ce"]
    assert kosire["intensity"]["value"] == before["value"]
    assert kosire["dateObservedTo"]["value"] == before["measured_to"].replace(".000Z", "Z")


@requires_docker
@pytest.mark.parametrize(
    ("counters", "detections", "status", "reason"),
    [
        ({"error_message": "Forbidden"}, DETECTIONS, 200, "not a feature list"),
        ({"type": "FeatureCollection", "features": []}, DETECTIONS, 200, "listed no cycle counter"),
        (COUNTERS, {"error_message": "Bad request"}, 200, "not a list"),
        (COUNTERS, [], 200, "counted no bicycle"),
        (COUNTERS, [{**DETECTIONS[0], "value": None}] * 3, 200, "counted no bicycle"),
        (COUNTERS, [DETECTIONS[0]] * 10000, 200, "the window is cut"),
        (COUNTERS, {"error_message": "Forbidden", "error_status": 403}, 403, "403"),
    ],
    ids=["counters-not-a-list", "no-counter", "counts-not-a-list", "no-count", "only-empty-counts", "cut", "refused"],
)
def test_a_broken_answer_fails_the_run_instead_of_emptying_the_counts(counters, detections, status, reason):
    entities, errors = outcome(counters=counters, detections=detections, status=status)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


@requires_docker
def test_a_counter_list_not_updated_for_a_day_fails_the_run():
    entities, errors = outcome(now="2026-10-26T15:26:00Z")
    assert entities == [] and len(errors) == 1 and "older than a day" in errors[0], errors


def test_the_pipeline_reads_golemio_by_the_secret_every_five_minutes_into_the_city_space():
    source = yaml.safe_load((PRAHA / "praha-datasource-bicycle-counters.yaml").read_text())
    http = source["spec"]["http"]
    assert http["url"] == "https://api.golemio.cz/v2/bicyclecounters"
    assert http["authorization"]["headerRef"] == {"name": "golemio", "key": "token"}
    pipeline = yaml.safe_load((PRAHA / "praha-pipeline-bicycle-counters.yaml").read_text())
    assert pipeline["spec"]["period"] == "5m"
    assert pipeline["spec"]["secretRefs"] == [{"name": "golemio", "key": "token", "envVar": "PRAHA_GOLEMIO_TOKEN"}]
    assert pipeline["spec"]["output"] == {"type": "TrafficFlowObserved", "mode": "upsert"}
    assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
    (fetch,) = [p["branch"]["processors"][0]["http"] for p in yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][0]["try"] if "branch" in p]
    assert fetch["url"].startswith(f"{SERVICE}/v2/bicyclecounters/detections?")
    assert fetch["headers"]["X-Access-Token"] == "${PRAHA_GOLEMIO_TOKEN}"
    assert fetch["timeout"] == "30s"
