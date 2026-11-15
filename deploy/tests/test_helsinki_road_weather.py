"""The Helsinki road-weather mapping over a recorded Digitraffic answer (T-5020, PL-03, DM-01).

Bento runs the committed processors over `fixtures/helsinki/digitraffic-road-weather.json`: three
stations of Fintraffic's answer to the `digitraffic-road-weather` DataSource, fetched on
2026-10-21 and cut to the sensors the mapping reads. 1001 lies outside the Helsinki box, 1002 is a
working station inside it, and 1071 is one Fintraffic lists with no sensors and the epoch as its
update time, as it answered for 1071, 1072, 1073, 1113 and 19966 that day.

Without a container runtime the Bento cases skip.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-helsinki-road-weather")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-road-weather-bento.yaml"
RECORDED = Path(__file__).resolve().parent / "fixtures/helsinki/digitraffic-road-weather.json"
# The digest the pipeline runner pins (test_helsinki_air_quality.py).
BENTO = "ghcr.io/warpstreamlabs/bento:1.22.0@sha256:ef674bebae45793c982c350d8f37c2c7661133c1dc65a5517fb977b36dc09841"
PREFIX = "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:road-"


def run(document: str) -> list[dict]:
    """The mapping's processors over one fetched document, as the runner feeds it."""
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"],
        "output": {"stdout": {"codec": "all-bytes"}},
        "logger": {"level": "error"},
    }
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, text=True, timeout=120,
    )
    assert result.returncode == 0, result.stderr or result.stdout
    return json.loads(result.stdout)


@pytest.fixture(scope="module")
def recorded():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime, so Bento cannot run the mapping")
    return {entity["id"]: entity for entity in run(RECORDED.read_text())}


def test_only_the_working_station_inside_the_box_is_written(recorded):
    # 1001 is outside the box; 1071 reports nothing and would read as observed in 1970.
    assert sorted(recorded) == [PREFIX + "1002"]


def test_a_working_station_carries_its_readings_and_observation_time(recorded):
    station = recorded[PREFIX + "1002"]
    assert station["dateObserved"] == {"type": "Property", "value": "2026-10-21T12:36:49Z"}
    assert station["temperature"]["value"] == 11.0
    assert station["temperature"]["observedAt"] == "2026-10-21T12:35:48Z"
    assert station["location"]["value"]["coordinates"] == [24.898476, 60.166907]
