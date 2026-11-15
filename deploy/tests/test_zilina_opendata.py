"""The City of Žilina's own open data portal, opendata.zilina.sk, as live datasets (T-5232).

Each committed mapping runs in the pinned Bento over answers recorded from the portal on
2026-10-22 (tests/fixtures/zilina-opendata, cut to a few records each). The calls the counter
pipeline makes itself go to a stand-in on this machine that answers from the same recordings,
or with the 422 and 5xx the portal sends, so a mapping checked here is a mapping that ran. The
inventory test holds seeds/zilina-opendata/inventory.csv to the portal's 145 dataset pages.
"""

import csv
import http.server
import json
import subprocess
import threading
from collections import Counter
from pathlib import Path
from typing import ClassVar
from urllib.parse import urlsplit

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-zilina-opendata")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/zilina"
FIXTURES = Path(__file__).resolve().parent / "fixtures/zilina-opendata"
INVENTORY = ROOT / "seeds/zilina-opendata/inventory.csv"
RESOURCES = ROOT / "components/pipeline-runner/streams/resources.yaml"
SCHEMA = SEED / "zilina-doprava.v1.schema.json"
PORTAL = "https://opendata.zilina.sk"
SPACE = "zilina-doprava"
# The pipeline -> (its DataSource's recording, its output type).
PIPELINES = {
    "parkovanie": ("parking-list.json", "OffStreetParking"),
    "mhd-vozidla": ("vehicles.json", "Vehicle"),
    "scitace": ("summary.json", "TrafficFlowObserved"),
    "svietidla": ("street-lights.json", "Streetlight"),
}
# The hour the recordings were made in: the last full hour is 12:00 to 13:00 UTC.
NOW = "2026-10-22T13:20:00Z"


def manifest(name: str) -> dict:
    return yaml.safe_load((SEED / name).read_text())


def bento(pipeline: str) -> Path:
    return SEED / f"zilina-pipeline-{pipeline}-bento.yaml"


class Portal(http.server.BaseHTTPRequestHandler):
    """opendata.zilina.sk answering from the recordings. `fail` maps a path to the status it
    answers instead; `seen` keeps every body the pipeline posted."""

    fail: ClassVar[dict[str, int]] = {}
    oversize: ClassVar[set[str]] = set()
    seen: ClassVar[list[tuple[str, dict]]] = []

    def do_POST(self):
        path = urlsplit(self.path).path
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        Portal.seen.append((path, body))
        status = Portal.fail.get(path)
        if status:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"errors": [{"status": status, "title": "recorded failure"}]}).encode())
            return
        recordings = {
            "/api/datasets/v1/core/geojson/items": "counters.json",
            "/api/datasets/v1/traffic-counters/vehicles/hourly": "hourly.json",
        }
        if path in Portal.oversize:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b"[" + b"0," * 1_100_000 + b"0]")
            return
        if path not in recordings or (path.endswith("/items") and body != {"classes": "TrafficCounter"}):
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write((FIXTURES / recordings[path]).read_bytes())

    def log_message(self, *_):
        pass


def run(pipeline: str, document: bytes, fail: dict[str, int] | None = None, oversize: frozenset[str] = frozenset(),
        now: str = NOW) -> tuple[list[dict], list[str]]:
    """The mapping over `document`, its clock set to NOW and its portal calls answered by `Portal`.
    Returns the entities written and the errors of the messages the runner would refuse."""
    Portal.fail, Portal.oversize, Portal.seen = fail or {}, set(oversize), []
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Portal)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = bento(pipeline).read_text()
        text = text.replace(f"{PORTAL}/api/", f"http://127.0.0.1:{server.server_port}/api/")
        text = text.replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
        processors = yaml.safe_load(text)["pipeline"]["processors"]
        # The runner refuses an errored message (the run fails, nothing is written): shown here.
        processors.append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": {"processors": processors},
            "rate_limit_resources": yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "none"},
        }
        done = subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", f"JC_SPACE={SPACE}",
             "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
             'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=document, capture_output=True, timeout=180, check=False,
        )
    finally:
        server.shutdown()
    assert done.returncode == 0, done.stderr.decode()
    entities, errors = [], []
    for line in done.stdout.decode().splitlines():
        if not line.strip():
            continue
        message = json.loads(line)
        if isinstance(message, dict) and "__error" in message:
            errors.append(message["__error"])
        else:
            entities.extend(message if isinstance(message, list) else [message])
    return entities, errors


def recorded(pipeline: str) -> bytes:
    return (FIXTURES / PIPELINES[pipeline][0]).read_bytes()


# --- the inventory ---------------------------------------------------------------------------


def test_the_inventory_accounts_for_every_dataset_of_the_portal():
    rows = list(csv.DictReader(INVENTORY.open(encoding="utf-8")))
    # 145 dataset pages on the portal's 15 list pages, read 2026-10-22.
    assert len(rows) == 145 and len({r["id"] for r in rows}) == 145
    assert Counter(r["decision"] for r in rows) == {"taken": 4, "merged": 76, "skipped": 65}
    for row in rows:
        assert row["reason"].strip(), f"dataset {row['id']} has no reason"
        assert row["live"] in {"live", "static"}, row["id"]
    taken = {r["id"]: r for r in rows if r["decision"] == "taken"}
    assert set(taken) == {"57", "102", "124", "182"}
    for row in taken.values():
        assert row["licence"] == "Open Database License (ODbL)", row["id"]
        assert row["reason"].split(" ")[0] in PIPELINES, row["id"]


# --- the outbound calls ----------------------------------------------------------------------


def test_every_source_is_the_portal_without_credentials_and_with_a_timeout():
    for pipeline in PIPELINES:
        spec = manifest(f"zilina-pipeline-{pipeline}.yaml")["spec"]
        source = spec["source"]["dataSourceRef"]["name"]
        (path,) = [p for p in SEED.glob("zilina-datasource-*.yaml") if yaml.safe_load(p.read_text())["metadata"]["name"] == source]
        http = yaml.safe_load(path.read_text())["spec"]["http"]
        assert http["url"].startswith(f"{PORTAL}/api/datasets/"), path.name
        assert http["verb"] == "POST" and http["timeout"] in {"30s", "60s"}, path.name
        assert "authorization" not in http, "the portal needs no key, so none is sent"
        assert spec["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:zilina.sk:{SPACE}:{SPACE}"
        assert spec["output"]["type"] == PIPELINES[pipeline][1]


def test_the_calls_a_mapping_makes_itself_go_only_to_the_portal_once_a_second():
    def http_steps(node):
        if isinstance(node, dict):
            yield from ([node["http"]] if "http" in node else [])
            for value in node.values():
                yield from http_steps(value)
        elif isinstance(node, list):
            for item in node:
                yield from http_steps(item)

    calls = [c for p in PIPELINES for c in http_steps(yaml.safe_load(bento(p).read_text())["pipeline"])]
    assert len(calls) == 2
    for call in calls:
        assert call["url"].startswith(f"{PORTAL}/api/datasets/v1/")
        assert call["timeout"] == "30s" and call["rate_limit"] == "zilina_opendata"
        assert "oauth2" not in call and "basic_auth" not in call
    rates = {r["label"]: r["local"] for r in yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"]}
    assert rates["zilina_opendata"] == {"count": 1, "interval": "1s"}


# --- the mappings ----------------------------------------------------------------------------


@requires_docker
@pytest.mark.parametrize("pipeline", list(PIPELINES))
def test_each_recorded_answer_becomes_valid_entities_with_stable_ids_and_the_citys_credit(pipeline):
    first, errors = run(pipeline, recorded(pipeline))
    assert not errors and first, errors
    again, _ = run(pipeline, recorded(pipeline))
    assert [e["id"] for e in first] == [e["id"] for e in again], "an id is the portal's own, not the run's"
    assert len({e["id"] for e in first}) == len(first)
    for entity in first:
        assert entity["type"] == PIPELINES[pipeline][1]
        assert entity["id"].startswith(f"urn:ngsi-ld:{entity['type']}:zilina.sk:{SPACE}:")
        assert entity["dataProvider"]["value"] == "Mesto Žilina, ODbL"
        assert entity["source"]["value"].startswith(f"{PORTAL}/sk/dataset-list/detail/")
        assert open_data.schema_errors(SCHEMA, entity) == [], (entity["id"], open_data.schema_errors(SCHEMA, entity))


@requires_docker
def test_a_car_park_carries_its_free_and_total_spaces_now():
    (park,), _ = run("parkovanie", recorded("parkovanie"))
    assert park["id"].endswith(":invipo-67b5e8c335c59400017efe4b")
    assert (park["availableSpotNumber"]["value"], park["totalSpotNumber"]["value"], park["occupiedSpotNumber"]["value"]) == (52, 62, 10)
    assert park["availableSpotNumber"]["observedAt"] == park["dateObserved"]["value"] == NOW


@requires_docker
def test_a_vehicle_carries_its_delay_in_seconds_and_expires_after_three_missed_reads():
    vehicles, _ = run("mhd-vozidla", recorded("mhd-vozidla"))
    source = json.loads(recorded("mhd-vozidla"))
    assert len(vehicles) == len(source) - 1, "the vehicle without a position is left out"
    first = vehicles[0]
    assert first["id"].endswith(":dpmz-284-koda26tr-solaris")
    assert first["delay"] == {"type": "Property", "value": 192, "unitCode": "SEC", "observedAt": "2026-10-22T13:37:00Z"}
    assert first["expiresAt"] == "2026-10-22T13:52:00Z"
    assert any(v["delay"]["value"] < 0 for v in vehicles), "a vehicle ahead of time keeps the sign"


@requires_docker
def test_a_counter_carries_the_last_full_hour_from_its_own_call():
    counters, _ = run("scitace", recorded("scitace"))
    active = [f for f in json.loads((FIXTURES / "counters.json").read_text())
              if f["properties"]["item"]["operatingStatus"] == "Active"]
    assert len(counters) == len(active) == 3, "the disabled counter is not asked"
    asked = [body for path, body in Portal.seen if path.endswith("/hourly")]
    assert asked[0] == {"from": "2026-10-22T12:00:00.000Z", "to": "2026-10-22T13:00:00.000Z", "itemId": active[0]["properties"]["item"]["id"]}
    first = counters[0]
    assert first["intensity"] == {"type": "Property", "value": 371, "observedAt": "2026-10-22T13:00:00Z"}
    assert first["averageVehicleSpeed"]["value"] == 49.9
    assert (first["dateObservedFrom"]["value"], first["dateObservedTo"]["value"]) == ("2026-10-22T12:00:00Z", "2026-10-22T13:00:00Z")


# --- failures leave the last entities in place -----------------------------------------------


@requires_docker
@pytest.mark.parametrize("pipeline,answer", [
    ("parkovanie", b"[]"),
    ("parkovanie", b'{"errors":[{"status":500}]}'),
    ("mhd-vozidla", b'{"errors":[{"status":422}]}'),
    ("svietidla", b"[]"),
    ("scitace", b'{"errors":[{"status":503}]}'),
])
def test_an_empty_or_refused_answer_fails_the_run_and_writes_nothing(pipeline, answer):
    entities, errors = run(pipeline, answer)
    assert entities == [] and errors, "the run fails where it can be seen and the last entities stay"


@requires_docker
@pytest.mark.parametrize("status", [422, 500])
def test_a_counter_register_the_portal_refuses_fails_the_run(status):
    entities, errors = run("scitace", recorded("scitace"), {"/api/datasets/v1/core/geojson/items": status})
    assert entities == [] and errors


@requires_docker
@pytest.mark.parametrize("status", [422, 502])
def test_a_counter_whose_hour_is_refused_is_left_out_and_keeps_its_last_hour(status):
    entities, errors = run("scitace", recorded("scitace"), {"/api/datasets/v1/traffic-counters/vehicles/hourly": status})
    assert entities == [] and errors == [], "no counter is written and the run does not fail the others"


@requires_docker
def test_an_empty_night_writes_no_vehicle_and_does_not_fail():
    assert run("mhd-vozidla", b"[]") == ([], [])


@requires_docker
@pytest.mark.parametrize("pipeline,cap,items", [("parkovanie", 1048576, 400), ("mhd-vozidla", 1048576, 400), ("svietidla", 16777216, 2000)])
def test_an_answer_over_its_size_cap_fails_the_run_and_writes_nothing(pipeline, cap, items):
    # Long texts: as many as every count cap allows, over the byte cap together.
    entities, errors = run(pipeline, json.dumps(["x" * (cap // items + 1)] * items).encode())
    assert entities == [] and errors and f"more than {cap}" in errors[0]


@requires_docker
def test_a_counter_register_over_its_size_cap_fails_the_run():
    entities, errors = run("scitace", recorded("scitace"), oversize=frozenset({"/api/datasets/v1/core/geojson/items"}))
    assert entities == [] and errors and "more than 2097152" in errors[0]


@requires_docker
def test_a_counter_hour_over_its_size_cap_is_left_out():
    oversize = frozenset({"/api/datasets/v1/traffic-counters/vehicles/hourly"})
    assert run("scitace", recorded("scitace"), oversize=oversize) == ([], [])


# --- history: on change, never per read ------------------------------------------------------


def recorded_values(entity: dict) -> dict:
    """What record: onChange compares: each attribute's value, without its observedAt and
    without the attributes the space records never."""
    never = {name for name, rule in retention(entity["type"]).get("attributes", {}).items() if rule == {"record": "never"}}
    return {k: v.get("value", v.get("languageMap", v.get("object"))) if isinstance(v, dict) else v
            for k, v in entity.items() if k not in never}


def retention(kind: str) -> dict:
    return {r["entityType"]: r for r in manifest("zilina-space-doprava.yaml")["spec"]["retention"]}[kind]


@requires_docker
def test_an_equal_reading_writes_no_history_row_and_a_changed_one_writes_one():
    first, _ = run("parkovanie", recorded("parkovanie"))
    again, _ = run("parkovanie", recorded("parkovanie"), now="2026-10-22T13:25:00Z")
    assert first[0]["dateObserved"] != again[0]["dateObserved"], "the read's own time moved"
    assert recorded_values(first[0]) == recorded_values(again[0]), "an equal count records no row"
    source = json.loads(recorded("parkovanie"))
    source[0]["occupancy"]["overall"]["free"] = 51
    changed, _ = run("parkovanie", json.dumps(source).encode(), now="2026-10-22T13:25:00Z")
    differ = {k for k, v in recorded_values(changed[0]).items() if recorded_values(first[0]).get(k) != v}
    assert differ == {"availableSpotNumber", "occupiedSpotNumber"}, "one changed count records one row per attribute"



def test_live_counts_are_recorded_on_change_and_hours_once_each():
    space = manifest("zilina-space-doprava.yaml")["spec"]
    rules = {r["entityType"]: r for r in space["retention"]}
    for kind in ("OffStreetParking", "Vehicle"):
        # An equal count writes no row; a changed one writes one; an hour of no change writes one.
        assert (rules[kind]["record"], rules[kind]["heartbeat"]) == ("onChange", "PT1H"), kind
        # The read's own time changes every run and is the current state only.
        assert rules[kind]["attributes"]["dateObserved"] == {"record": "never"}, kind
    # An hour's count carries the hour's end as observedAt: a rerun of the same hour writes nothing.
    assert rules["TrafficFlowObserved"]["record"] == "observed"
    assert rules["Streetlight"]["history"] == "off"


def test_each_kind_is_one_public_odbl_dataset_crediting_the_city():
    for name, kind in {"parkoviska": "OffStreetParking", "mhd": "Vehicle", "scitace": "TrafficFlowObserved", "svietidla": "Streetlight"}.items():
        endpoint = manifest(f"zilina-endpoint-{name}.yaml")["spec"]
        policy = manifest(f"zilina-policy-{name}-read.yaml")["spec"]
        assert endpoint["policyRef"] == f"urn:ngsi-ld:Policy:zilina.sk:{SPACE}:{name}-read"
        assert [e["type"] for i in policy["information"] for e in i["entities"]] == [kind]
        assert not {"upsertBatch", "createBatch", "deleteBatch"} & set(policy["operations"]), "the public only reads"
        catalog = endpoint["catalog"]
        assert catalog["license"] == "ODC_ODBL"
        # A dev-empty Endpoint leaves the catalogue until it holds data (T-5767); the rest publish under ODbL.
        if catalog.get("harvest") is False:
            assert "publish" not in endpoint
        else:
            assert endpoint["publish"]["ckan"]["license"] == "odc-odbl"
        assert catalog["publisher"]["uri"] == "https://data.gov.sk/id/legal-subject/00321796"
        assert "Mesto Žilina" in catalog["attribution"]["sk"] and "ODbL" in catalog["attribution"]["en"]
