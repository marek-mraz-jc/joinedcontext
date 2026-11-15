"""Forum Virium Helsinki's street cleaning air quality pilot in the Helsinki space (T-5255, PL-03).

Bento runs the committed mapping over a recorded answer of the DataSource's URL (fetched on
2026-10-22, three readings of each of the three devices, each device's `locations` track cut to two points, which the mapping never reads), gzipped as the source serves it. The
recording also holds real readings the plausibility rules drop: a washer reading with no GPS fix
(null), a brush machine reading at 0,0, a brush machine PM10 of 1371 µg/m³ and a Rastila reading
whose PM2.5 exceeds its PM10; two are edited in one place each: a reading moved to Tapiola
(Espoo, outside Helsinki) and a Rastila reading repeated. Every entity is checked against the
space's generated JSON Schema and against Smart Data Models' AirQualityObserved at its pin.
"""

import copy
import gzip
import json
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-street-cleaning-air-bento.yaml"
RECORDING = Path(__file__).resolve().parent / "fixtures/helsinki/open-data/swarco-street-cleaning.json"


def run(devices: list) -> list[dict]:
    return open_data.run_all(MAPPING, gzip.compress(json.dumps(devices).encode()), "helsinki")


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return run(json.loads(RECORDING.read_text()))


@requires_docker
def test_every_plausible_reading_is_one_air_quality_observation(written):
    assert {e["type"] for e in written} == {"AirQualityObserved"}
    # 15 readings: 3 per device are plausible; the 6 others are dropped.
    assert len(written) == 9
    ids = [e["id"] for e in written]
    assert len(ids) == len(set(ids))
    for entity in written:
        assert re.fullmatch(r"urn:ngsi-ld:AirQualityObserved:hel\.fi:helsinki:swarco-[0-9]+-[0-9]+", entity["id"])


@requires_docker
def test_every_reading_is_one_the_gateway_and_the_smart_data_model_accept(written):
    for entity in written:
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity["id"]
        assert not open_data.sdm_errors("dataModel.Environment/AirQualityObserved", entity), entity["id"]


@requires_docker
def test_a_reading_keeps_its_place_time_and_both_particle_sizes_in_micrograms(written):
    reading = next(e for e in written if e["id"].endswith("swarco-89011702278837491871-10936"))
    assert reading["location"]["value"] == {"type": "Point", "coordinates": [25.155269622802734, 60.21601486206055]}
    assert reading["dateObserved"]["value"] == "2023-04-11T03:29:40Z"
    assert reading["pm10"] == {"type": "Property", "value": 4.657, "unitCode": "GQ", "observedAt": "2023-04-11T03:29:40Z"}
    assert reading["pm25"]["value"] == 4.139667
    assert reading["name"]["languageMap"] == {"fi": "Volvo (pesuauto)"}
    assert reading["source"]["value"].startswith("https://dl.fvh.fi/")


@requires_docker
def test_every_implausible_reading_is_dropped(written):
    devices = json.loads(RECORDING.read_text())
    kept = {e["id"].rsplit("-", 1)[1] for e in written}
    # No GPS fix (null and 0,0), PM10 1371, PM2.5 above PM10, the Tapiola point.
    for device, reading in ((0, 3), (1, 3), (1, 4), (2, 3), (0, 4)):
        assert str(devices[device]["data_points"][reading]["id"]) not in kept
    for entity in written:
        lon, lat = entity["location"]["value"]["coordinates"]
        assert 24.80 <= lon <= 25.28 and 59.91 <= lat <= 60.31, entity["id"]
        assert 0 <= entity["pm25"]["value"] <= entity["pm10"]["value"] <= 1000, entity["id"]


@requires_docker
def test_a_reading_dated_ahead_and_one_without_id_are_dropped():
    devices = json.loads(RECORDING.read_text())[:1]
    devices[0]["data_points"] = devices[0]["data_points"][:2]
    devices[0]["data_points"][0]["timestamp"] = "2099-01-01T00:00:00+02:00"
    del devices[0]["data_points"][1]["id"]
    assert run(devices) == []


@requires_docker
def test_an_empty_file_fails_the_run_rather_than_writing_nothing():
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"],
              "output": {"stdout": {"codec": "all-bytes"}}, "logger": {"level": "error"}}
    for empty in ([], [{"id": "1", "name": "x", "data_points": []}]):
        result = subprocess.run(
            ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki", "-e", "CONFIG=" + json.dumps(config),
             "--entrypoint", "sh", open_data.BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=gzip.compress(json.dumps(empty).encode()), capture_output=True, timeout=120,
        )
        assert b"the pilot file holds no reading" in result.stdout + result.stderr, empty


def test_the_run_logs_how_many_readings_it_dropped_and_why():
    pipeline = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    mapping = "\n".join(step["mapping"] for step in pipeline if "mapping" in step)
    for reason in ("no position", "outside Helsinki", "PM out of range", "no valid time or id", "duplicate"):
        assert f'"{reason}"' in mapping, reason
    (log,) = [step["log"] for step in pipeline if "log" in step]
    assert log["message"].count("${! @") == 3


def test_the_pilot_is_seeded_as_a_datasource_a_pipeline_a_mapping_and_a_public_endpoint():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    datasource = yaml.safe_load((HELSINKI / "helsinki-datasource-street-cleaning-air.yaml").read_text())
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-street-cleaning-air.yaml").read_text())
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-street-cleaning-air.yaml").read_text())
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-street-cleaning-air.yaml").read_text())
    assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"]
    assert pipeline["spec"]["output"]["type"] == "AirQualityObserved"
    assert pipeline["spec"]["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:helsinki:helsinki-all"
    assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 256
    assert datasource["spec"]["http"]["url"].startswith("https://dl.fvh.fi/")
    for seeded in ("helsinki-datasource-street-cleaning-air.yaml", "helsinki-pipeline-street-cleaning-air.yaml",
                   "helsinki-endpoint-street-cleaning-air.yaml", "helsinki-policy-public-street-cleaning-air.yaml"):
        assert seeded in index, seeded
    assert index["helsinki-pipeline-street-cleaning-air-bento.yaml"] == "projects/helsinki/pipelines/street-cleaning-air/bento.yaml"
    # The public grant reads the pilot's readings only: FMI's stations stay behind the app's grant.
    (rule,) = policy["spec"]["information"]
    (selector,) = rule["entities"]
    assert selector["type"] == "AirQualityObserved"
    assert re.fullmatch(selector["idPattern"], "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:swarco-1-2")
    assert not re.fullmatch(selector["idPattern"], "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:fmi-kallio-2")
    assert set(rule["propertyNames"]) == {"dateObserved", "location", "name", "pm10", "pm25", "source"}
    assert endpoint["spec"]["policyRef"].endswith(":public-street-cleaning-air")
    assert {"ngsi-ld", "csv", "ogc-features", "odata"} <= set(endpoint["spec"]["enabledRepresentations"])
    assert endpoint["spec"]["publish"]["ckan"]["instanceRef"]["name"] == "hel-fi"
    assert endpoint["spec"]["catalog"]["pipelineRef"]["name"] == "street-cleaning-air"
    # What the model leaves out is named in the catalogue entry.
    assert "temperature" in endpoint["metadata"]["description"]["en"]


def test_the_space_grants_the_pipeline_writes_and_keeps_no_wider_grant():
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-pipelines-write.yaml").read_text())
    assert "deleteBatch" not in policy["spec"]["operations"]
    unchanged = copy.deepcopy(policy["spec"]["operations"])
    assert unchanged == ["upsertBatch", "createBatch", "queryBatch"]
