"""SHMÚ's automatic weather stations into `banskabystrica-verejne` (T-4153, PL-03, DM-61).

Bento runs the committed mapping with its three requests to opendata.shmu.sk answered on this
machine from recordings of 2026-10-15 (`fixtures/banskabystrica/shmu-*`): the data folder's
index, the day's index cut to three files, and the 21:35 file cut to three stations, the city's
two and one elsewhere. Every entity it writes is checked against the space's generated JSON
Schema, which the gateway enforces.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import base64
import hashlib
import http.server
import json
import re
import shutil
import subprocess
import threading
import urllib.parse
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-banskabystrica-weather")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/banskabystrica"
FIXTURES = Path(__file__).resolve().parent / "fixtures/banskabystrica"
MAPPING = CITY / "pipeline-pocasie-bento.yaml"
SPACE = "banskabystrica-verejne"
DOMAIN = "banskabystrica.sk"
HOST = "https://opendata.shmu.sk"
DATA = "/meteorology/climate/now/data/"
FILE = "aws1min - 2026-10-15 21-35-00.json"
# Sectigo Public Server Authentication CA DV R36, as the mapping's comment names it.
PINNED_SHA256 = "8C54C334B66BA4E426772AF4A3F9136C19A1AEC729FDB28C535C07A5A4EF22E0"


class Recorded(http.server.BaseHTTPRequestHandler):
    """opendata.shmu.sk as recorded; `answers` replaces a path's body for one case."""

    answers: dict[str, bytes] = {}

    def do_GET(self):
        path = urllib.parse.unquote(urllib.parse.urlsplit(self.path).path)
        recorded = {
            DATA: FIXTURES / "shmu-data-index.html",
            DATA + "20261015/": FIXTURES / "shmu-day-index.html",
            DATA + "20261015/" + FILE: FIXTURES / "shmu-aws1min.json",
        }
        if path in self.answers:
            body = self.answers[path]
        elif path in recorded:
            body = recorded[path].read_bytes()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json" if path.endswith(".json") else "text/html")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def pocasie(answers: dict[str, bytes] | None = None) -> subprocess.CompletedProcess:
    Recorded.answers = answers or {}
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Recorded)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = MAPPING.read_text().replace(HOST, f"http://127.0.0.1:{server.server_port}")
        config = {
            # What the generate source hands the mapping: one empty message per run.
            "input": {"generate": {"mapping": 'root = ""', "count": 1, "interval": ""}},
            "pipeline": yaml.safe_load(text)["pipeline"],
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "all-bytes"}},
            "logger": {"level": "error"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "--network", "host", "-e", f"JC_ORG_DOMAIN={DOMAIN}",
             "-e", f"JC_SPACE={SPACE}", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def recorded_file() -> dict:
    return json.loads((FIXTURES / "shmu-aws1min.json").read_text())


def latest(station: int, rows: list[dict]) -> dict:
    return max((r for r in rows if r["ind_kli"] == station), key=lambda r: r["minuta"])


def manifest(name: str) -> dict:
    return yaml.safe_load((CITY / name).read_text())


@pytest.fixture(scope="module")
def stations():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    result = pocasie()
    assert result.returncode == 0, result.stderr.decode()
    return {e["id"].rsplit(":", 1)[1]: e for e in json.loads(result.stdout)}


@requires_docker
def test_the_citys_two_stations_and_no_other_are_written(stations):
    assert sorted(stations) == ["shmu-11894", "shmu-11898"], "11800 is in Bratislava"
    for local, entity in stations.items():
        assert entity["id"] == f"urn:ngsi-ld:WeatherObserved:{DOMAIN}:{SPACE}:{local}"
        assert entity["type"] == "WeatherObserved"
        assert not open_data.schema_errors(CITY / "banskabystrica-verejne.v1.schema.json", entity)
        assert open_data.sdm_errors("dataModel.Weather/WeatherObserved", entity) == [], entity["id"]
    assert stations["shmu-11898"]["name"]["languageMap"]["sk"] == "Stanica SHMÚ Banská Bystrica - Zelená"
    assert stations["shmu-11898"]["location"]["value"]["coordinates"] == [19.115097, 48.733419]


@requires_docker
def test_each_station_reports_its_latest_minute_in_utc_and_the_models_units(stations):
    rows = recorded_file()["data"]
    for code in (11898, 11894):
        row, entity = latest(code, rows), stations[f"shmu-{code}"]
        # A summer minute is UTC+1: 20:30 is 19:30 UTC.
        hour = int(row["minuta"][11:13]) - 1
        at = f"{row['minuta'][:11]}{hour:02d}{row['minuta'][13:]}Z"
        assert entity["dateObserved"]["value"] == at
        assert entity["temperature"] == {"type": "Property", "value": row["t"], "unitCode": "CEL", "observedAt": at}
        assert entity["relativeHumidity"]["value"] == round(row["vlh_rel"] / 100, 3)
        assert entity["atmosphericPressure"]["value"] == row["tlak"]
        assert entity["windSpeed"]["value"] == row["vie_pr_rych"]
        assert entity["windDirection"]["value"] == row["vie_vp_smer"]
        assert entity["precipitation"]["value"] == 0
        assert entity["dataProvider"]["value"].startswith("Slovenský hydrometeorologický ústav")


def at_minute(minute: str) -> subprocess.CompletedProcess:
    """The recorded file with Zelená's only row at `minute`."""
    document = recorded_file()
    row = dict(latest(11898, document["data"]), minuta=minute)
    document["data"] = [r for r in document["data"] if r["ind_kli"] != 11898] + [row]
    return pocasie({DATA + "20261015/" + FILE: json.dumps(document).encode()})


@requires_docker
@pytest.mark.parametrize(("minute", "utc"), [
    ("2026-10-25T00:30:00", "2026-10-24T23:30:00Z"),  # 01:30 CEST, before the change
    ("2026-10-25T01:30:00", "2026-10-25T01:30:00Z"),  # 02:30 CET, the repeated hour, read after it
    ("2026-10-25T10:13:00", "2026-10-25T10:13:00Z"),  # 11:13 CET: measured on dev as 2 h behind
    ("2026-03-29T00:30:00", "2026-03-29T00:30:00Z"),  # 01:30 CET, before the spring change
    ("2026-03-29T02:30:00", "2026-03-29T01:30:00Z"),  # 03:30 CEST, after it
    ("2026-07-01T12:00:00", "2026-07-01T11:00:00Z"),
    ("2025-12-01T12:00:00", "2025-12-01T12:00:00Z"),
])
def test_the_minute_is_bratislava_time_minus_an_hour_on_both_sides_of_each_change(minute, utc):
    result = at_minute(minute)
    assert result.returncode == 0, result.stderr.decode()
    station = next(e for e in json.loads(result.stdout) if e["id"].endswith("shmu-11898"))
    assert station["dateObserved"]["value"] == utc
    assert station["temperature"]["observedAt"] == utc


@requires_docker
@pytest.mark.parametrize("minute", ["2026-03-29T01:30:00", "2099-01-01T12:00:00", "2099-07-01T12:00:00"])
def test_a_minute_no_clock_could_show_leaves_that_station_out(minute):
    """01:xx never occurs on the March change day; a minute ahead of now is no reading yet."""
    result = at_minute(minute)
    assert result.returncode == 0, result.stderr.decode()
    assert [e["id"].rsplit(":", 1)[1] for e in json.loads(result.stdout)] == ["shmu-11894"]


@requires_docker
def test_soil_temperature_is_one_instance_per_measured_depth(stations):
    row = latest(11898, recorded_file()["data"])
    soil = stations["shmu-11898"]["soilTemperature"]
    assert [i["datasetId"] for i in soil] == [f"urn:ngsi-ld:Dataset:shmu:soil-depth-{d}cm" for d in (5, 10, 20, 50, 100)]
    assert [i["value"] for i in soil] == [row[f"t_pod{d}"] for d in (5, 10, 20, 50, 100)]
    assert all(i["unitCode"] == "CEL" and i["observedAt"] == stations["shmu-11898"]["dateObserved"]["value"] for i in soil)
    # 11894 has no soil sensors: every t_pod is null, so the attribute is left out, not empty.
    assert "soilTemperature" not in stations["shmu-11894"]


def test_a_soil_instance_of_the_wrong_type_is_a_schema_error():
    entity = {"id": "urn:ngsi-ld:WeatherObserved:x", "type": "WeatherObserved", "soilTemperature": [
        {"type": "Property", "value": 8.2, "datasetId": "urn:ngsi-ld:Dataset:a"},
        {"type": "Property", "value": "warm", "datasetId": "urn:ngsi-ld:Dataset:b"},
        {"type": "Property", "value": 9.1, "datasetId": "urn:ngsi-ld:Dataset:c"}]}
    assert open_data.schema_errors(CITY / "banskabystrica-verejne.v1.schema.json", entity)


@requires_docker
def test_rain_is_the_sum_of_the_last_ten_minutes_only():
    document = recorded_file()
    rows = sorted((r for r in document["data"] if r["ind_kli"] == 11898), key=lambda r: r["minuta"])
    # 0.3 mm in the latest minute, 0.2 mm nine minutes before it, 5 mm eleven minutes before it.
    end = rows[-1]["minuta"]
    rows[-1]["zra_uhrn"] = 0.3
    extra = [dict(rows[-1], minuta=end[:14] + f"{int(end[14:16]) - m:02d}:00", zra_uhrn=v) for m, v in ((9, 0.2), (11, 5.0))]
    document["data"] = [r for r in document["data"] if r["ind_kli"] != 11898] + rows + extra
    result = pocasie({DATA + "20261015/" + FILE: json.dumps(document).encode()})
    assert result.returncode == 0, result.stderr.decode()
    station = next(e for e in json.loads(result.stdout) if e["id"].endswith("shmu-11898"))
    assert station["precipitation"]["value"] == 0.5


@requires_docker
def test_a_minute_without_a_reading_leaves_that_attribute_out_and_keeps_the_others():
    document = recorded_file()
    row = latest(11898, document["data"])
    row["t"] = None
    row["vlh_rel"] = 140.0
    result = pocasie({DATA + "20261015/" + FILE: json.dumps(document).encode()})
    assert result.returncode == 0, result.stderr.decode()
    station = next(e for e in json.loads(result.stdout) if e["id"].endswith("shmu-11898"))
    assert "temperature" not in station and "relativeHumidity" not in station
    assert station["windSpeed"]["value"] == row["vie_pr_rych"]



@requires_docker
def test_ten_minutes_without_a_rain_reading_write_no_precipitation_rather_than_zero():
    """An empty sum is 0 mm: a gauge that reported nothing read as a dry day (F-BNT1-016)."""
    document = recorded_file()
    for r in document["data"]:
        if r["ind_kli"] == 11898:
            r["zra_uhrn"] = None
    result = pocasie({DATA + "20261015/" + FILE: json.dumps(document).encode()})
    assert result.returncode == 0, result.stderr.decode()
    station = next(e for e in json.loads(result.stdout) if e["id"].endswith("shmu-11898"))
    assert "precipitation" not in station and "temperature" in station

@requires_docker
def test_a_day_folder_without_a_file_fails_the_run_and_writes_nothing():
    result = pocasie({DATA + "20261015/": b"<html><body><h1>Index of</h1></body></html>"})
    assert result.returncode != 0 or result.stdout.strip() in (b"", b"[]")
    assert b"holds no file yet" in result.stderr + result.stdout


@requires_docker
def test_a_file_without_the_citys_stations_fails_the_run():
    document = recorded_file()
    document["data"] = [r for r in document["data"] if r["ind_kli"] == 11800]
    result = pocasie({DATA + "20261015/" + FILE: json.dumps(document).encode()})
    assert result.returncode != 0 or result.stdout.strip() in (b"", b"[]")
    assert b"holds no station of the city" in result.stderr + result.stdout


def test_every_request_verifies_tls_against_the_pinned_intermediate():
    """The host sends no intermediate; the mapping pins it and never turns verification off."""
    processors = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    calls = [p["http"] for p in processors if "http" in p]
    calls += [q["http"] for p in processors if "branch" in p for q in p["branch"]["processors"] if "http" in q]
    assert len(calls) == 3
    for call in calls:
        assert call["url"].startswith(HOST + DATA)
        assert call["tls"]["enabled"] is True and "skip_cert_verify" not in call["tls"]
        pem = call["tls"]["root_cas"]
        der = base64.b64decode("".join(re.findall(r"^[A-Za-z0-9+/=]+$", pem, re.M)))
        assert hashlib.sha256(der).hexdigest().upper() == PINNED_SHA256


def test_the_source_the_pipeline_and_the_catalogue_agree():
    source, pipeline = manifest("datasource-pocasie.yaml"), manifest("pipeline-pocasie.yaml")
    assert source["spec"] == {"type": "generate", "input": {"mapping": 'root = ""', "count": 1, "interval": ""}}
    spec = pipeline["spec"]
    assert spec["source"]["dataSourceRef"]["name"] == source["metadata"]["name"]
    assert spec["output"] == {"type": "WeatherObserved", "mode": "upsert"}
    assert spec["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:{DOMAIN}:{SPACE}:{SPACE}"
    assert (spec["period"], spec["schedule"]) == ("5m", "2-59/5 * * * *")
    index = yaml.safe_load((CITY / "index.yaml").read_text())
    for seeded in ("datasource-pocasie.yaml", "pipeline-pocasie.yaml", "pipeline-pocasie-bento.yaml"):
        assert seeded in index, seeded
    for name in ("policy-verejne-read.yaml", "policy-verejne-pipelines-write.yaml"):
        types = {e["type"] for rule in manifest(name)["spec"]["information"] for e in rule["entities"]}
        assert "WeatherObserved" in types, name
    assert "WeatherObserved" in manifest("datamodel-verejne.yaml")["spec"]["classes"]
    # The weather is its own dataset (T-4344).
    endpoint = manifest("endpoint-pocasie.yaml")
    assert "Slovak Hydrometeorological Institute" in endpoint["spec"]["catalog"]["attribution"]["en"]
    assert endpoint["spec"]["publish"]["ckan"]["license"] == "cc-by"
