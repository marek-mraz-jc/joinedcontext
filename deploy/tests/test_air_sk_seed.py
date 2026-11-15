"""The national air quality (T-4340): the generated mapping over recorded EEA files of two stations,
the stations' locations, the week's backfill and its restart, the zone of the files' naive times,
and the seed's project around it."""

import http.server
import importlib.util
import json
import subprocess
import threading
from pathlib import Path

import jsonschema
import pytest
import yaml
from referencing import Registry, Resource

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-air-sk")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/slovensko"
FIXTURES = Path(__file__).resolve().parent / "fixtures/slovensko"
SDM = ROOT / "scripts/open-data/sdm"
BLOB = "https://eeadmz1batchservice02.blob.core.windows.net/airquality-p/SK/"

spec = importlib.util.spec_from_file_location("air_sk", ROOT / "scripts/air-sk/generate.py")
generate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generate)
STATIONS = generate.stations(ROOT / "scripts/air-sk/stations.tsv")
# The fixtures' last rows end at 2026-10-16 12:00 naive (UTC+1), 11:00Z; each file holds 24 hours.
NOW = "2026-10-16T12:37:00Z"


def run(urls: list[str], stored: object = (), now: str = NOW) -> subprocess.CompletedProcess:
    """The mapping for `urls`, its files served from the fixtures (any other a 404) and the
    Endpoint's stored stations answered with `stored` (None: a 500)."""
    class Server(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path.startswith("/api/endpoint/"):
                body = None if stored is None else json.dumps(list(stored)).encode()
            else:
                f = FIXTURES / self.path.rsplit("/", 1)[-1]
                body = f.read_bytes() if f.is_file() else b""
                if not f.is_file():
                    self.send_response(404)
                    self.end_headers()
                    return
            self.send_response(500 if body is None else 200)
            self.end_headers()
            self.wfile.write(body or b"")

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Server)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_port}"
        text = generate.render(STATIONS, generate.files(urls, STATIONS))
        pipeline = yaml.safe_load(text.replace("https://eeadmz1batchservice02.blob.core.windows.net", base).replace("${JC_GATEWAY_URL}", base))["pipeline"]
        pipeline["processors"].insert(0, {"mapping": f"meta jc_now = {json.dumps(now)}"})
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": pipeline,
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 1000, "interval": "1s"}}],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "info"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=ovzdusie-sk",
             "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", open_data.BENTO, "-c",
             'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=b"start", capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def written(result: subprocess.CompletedProcess) -> list[dict]:
    assert result.returncode == 0, result.stderr.decode()
    return [m for m in (json.loads(line) for line in result.stdout.decode().splitlines() if line.strip().startswith("{")) if "id" in m]


FILES = [BLOB + f.name for f in sorted(FIXTURES.glob("*.parquet"))]
# The publisher's domain, though the runner's JC_ORG_DOMAIN above is the installation's hel.fi (T-4373).
URN = "urn:ngsi-ld:AirQualityObserved:joinedcontext.com:ovzdusie-sk:"


@requires_docker
def test_the_first_run_writes_every_valid_hour_of_the_week_oldest_first_at_its_utc_time():
    out = written(run(FILES))
    by = {}
    for e in out:
        by.setdefault(e["id"], []).append(e)
    assert sorted(by) == [URN + "SK0001A", URN + "SK0016A"]
    bratislava = by[URN + "SK0001A"]
    # 24 hours in each fixture, their windows one hour apart: one entity per hour, PM10 and NO2
    # merged into it where both files have the hour.
    hours = [e["dateObserved"]["value"] for e in bratislava]
    assert len(hours) == len(set(hours)) == 25
    assert sum(1 for e in bratislava if "pm10" in e and "no2" in e) == 23
    assert [e["dateObserved"]["value"] for e in bratislava] == sorted(e["dateObserved"]["value"] for e in bratislava)
    last = bratislava[-1]
    # The file's last row ends at 12:00 naive, which is UTC+1: 11:00Z. A one-hour shift fails here.
    assert last["dateObserved"]["value"] == "2026-10-16T11:00:00Z"
    assert last["pm10"] == {"type": "Property", "value": 22.033, "unitCode": "GQ", "observedAt": "2026-10-16T11:00:00Z"}
    assert last["no2"]["value"] == 5.50981 and last["no2"]["observedAt"] == "2026-10-16T11:00:00Z"
    assert last["location"]["value"] == {"type": "Point", "coordinates": [17.125496, 48.124711]}
    assert last["name"] == {"type": "Property", "value": "Bratislava, stanica SK0001A (mestská pozaďová)"}
    # Just these values: no source columns and nothing about where it came from.
    assert set(last) == {"id", "type", "name", "location", "dateObserved", "pm10", "no2"}
    # Ozone's validity 4 is a valid reading.
    assert by[URN + "SK0016A"][-1]["o3"]["value"] == 61.35
    # Hour by hour, every station in each (T-4410): a batch repeats one id as few times as it can,
    # so the broker's rounds of repeats stay few and a cut batch still leaves whole hours.
    keys = [(e["dateObserved"]["value"], e["id"]) for e in out]
    ids = [e["id"] for e in out]
    assert keys == sorted(keys) and ids.index(URN + "SK0016A") < len(ids) - 1 - ids[::-1].index(URN + "SK0001A")


@requires_docker
def test_a_later_run_writes_only_the_hours_after_what_the_endpoint_holds():
    stored = [{"id": URN + "SK0001A", "type": "AirQualityObserved",
               "pm10": {"type": "Property", "value": 1, "observedAt": "2026-10-16T09:00:00Z"},
               "no2": {"type": "Property", "value": 1, "observedAt": "2026-10-16T11:00:00Z"}}]
    out = [e for e in written(run(FILES, stored)) if e["id"] == URN + "SK0001A"]
    assert [(e["dateObserved"]["value"], sorted(k for k in e if k in ("pm10", "no2"))) for e in out] == [
        ("2026-10-16T10:00:00Z", ["pm10"]), ("2026-10-16T11:00:00Z", ["pm10"])]


@requires_docker
def test_the_backfill_reaches_back_seven_days_and_no_further():
    # Eight days on, only the rows of the last seven days are written: none of the fixtures' day.
    late = written(run(FILES[:1], now="2026-10-23T10:30:00Z"))
    assert [e["dateObserved"]["value"] for e in late] == ["2026-10-16T11:00:00Z"]


@requires_docker
def test_a_missing_file_is_left_out_with_a_line_and_no_file_fails_the_run():
    result = run(FILES[:1] + [BLOB + "SPO-SK0002A_00005_100.parquet"])
    assert {e["id"] for e in written(result)} == {URN + "SK0001A"}
    assert b"left out" in result.stderr and b"SPO-SK0002A_00005_100" in result.stderr
    none = run([BLOB + "SPO-SK0002A_00005_100.parquet"])
    assert b"no station file came" in none.stderr + none.stdout
    assert written(none) == []


@requires_docker
def test_a_failed_read_of_the_endpoint_writes_nothing():
    result = run(FILES, stored=None)
    assert written(result) == []
    assert b"the stored stations could not be read" in result.stderr + result.stdout


def test_a_station_missing_from_the_metadata_is_left_out_with_its_reason(capsys):
    kept = generate.files([BLOB + "SPO-SK9999A_00005_100.parquet", BLOB + "SPO-SK0001A_00010_100.parquet", FILES[0]], STATIONS)
    assert kept == [{"url": FILES[0], "station": "SK0001A", "attribute": "pm10"}]
    err = capsys.readouterr().err
    assert "SK9999A has no location" in err and "pollutant 10 is none of" in err


def test_every_station_of_the_pinned_list_is_located_and_the_committed_mapping_is_generated():
    urls = (ROOT / "scripts/air-sk/files.txt").read_text().split()
    assert len(urls) == 159 and len(generate.files(urls, STATIONS)) == 159
    assert len({generate.FILE.match(u).group(1) for u in urls}) == 53
    for code, s in STATIONS.items():
        lon, lat = s["point"]
        assert 16.8 < lon < 22.6 and 47.7 < lat < 49.7, code
    result = subprocess.run(["python3", str(ROOT / "scripts/air-sk/generate.py"), "--check"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr


@requires_docker
def test_every_entity_is_the_official_smart_data_model():
    common = Resource.from_contents(json.loads((SDM / "common-schema.json").read_text()))
    registry = Registry().with_resources((u, common) for u in (
        "https://smart-data-models.github.io/data-models/common-schema.json",
        "https://raw.githubusercontent.com/smart-data-models/data-models/master/common-schema.json"))
    schema = json.loads((SDM / "Environment/AirQualityObserved/schema.json").read_text())
    validator = jsonschema.Draft7Validator(schema, registry=registry, format_checker=jsonschema.Draft7Validator.FORMAT_CHECKER)
    own = json.loads((SEED / "ovzdusie-sk.v1.schema.json").read_text())
    for e in written(run(FILES))[-3:]:
        assert [x.message for x in validator.iter_errors(open_data.key_values(e))] == []
        assert open_data.schema_errors(SEED / "ovzdusie-sk.v1.schema.json", e) == [], own.get("title")


def test_the_space_keeps_a_week_and_the_endpoint_serves_every_surface_without_naming_the_source():
    space = yaml.safe_load((SEED / "slovensko-space-ovzdusie.yaml").read_text())
    (rule,) = space["spec"]["retention"]
    assert {k: v for k, v in rule.items() if k != "attributes"} == {"entityType": "AirQualityObserved", "history": "P7D"}
    # What a station is, written without an observedAt, records a change and a daily row (T-4844).
    assert all(a == {"record": "onChange", "heartbeat": "P1D"} for a in rule["attributes"].values())
    endpoint = yaml.safe_load((SEED / "slovensko-endpoint-ovzdusie.yaml").read_text())
    assert endpoint["spec"]["audience"] == "public"
    assert {"ngsi-ld", "mcp", "websocket", "csv", "ogc-features"} <= set(endpoint["spec"]["enabledRepresentations"])
    named = json.dumps([endpoint["metadata"]["title"], endpoint["metadata"]["description"], space["metadata"]], ensure_ascii=False)
    assert not any(word in named for word in ("EEA", "SHMÚ", "aqicn", "Agentúra", "Agency"))
    assert "EEA" in endpoint["spec"]["catalog"]["attribution"]["en"]
    assert generate.SLUG == endpoint["spec"]["slug"]
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    # The susr-* folders are placed by scripts/open-data/datacube.py (T-4402), checked in test_datacube_generator.
    assert sorted(k for k in index if not k.startswith("susr-")) == sorted(p.name for p in SEED.iterdir() if p.is_file() and p.name != "index.yaml")


def message_urls(value) -> list[str]:
    """Every `url`/`urls` value taken whole from the message: what jc-core's PL-07 check refuses."""
    if isinstance(value, list):
        return [u for v in value for u in message_urls(v)]
    if not isinstance(value, dict):
        return []
    found = []
    for key, inner in value.items():
        if key == "url" and isinstance(inner, str) and inner.lstrip().startswith("${!"):
            found.append(inner)
        if key == "urls" and isinstance(inner, list):
            found += [u for u in inner if isinstance(u, str) and u.lstrip().startswith("${!")]
        found += message_urls(inner)
    return found


def test_no_seeded_pipeline_takes_its_url_whole_from_the_message():
    # T-4373: dev refused ovzdusie (RunnerRefused, PL-07) for `url: "${! @url }"`.
    refused = {}
    for path in sorted(ROOT.glob("components/context-gateway/seed/*/*bento*.yaml")):
        if found := message_urls(yaml.safe_load(path.read_text())):
            refused[path.name] = found
    assert refused == {}
