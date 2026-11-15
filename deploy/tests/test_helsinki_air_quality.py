"""The Helsinki air-quality mapping over a recorded FMI answer (T-2617, PL-03, DM-01).

Bento runs the committed processors, XML step included, over `fixtures/helsinki/
fmi-airquality-hourly.xml`: FMI's answer to the URL of the `fmi-air-quality` DataSource, fetched
on 2026-09-22 with `starttime` three hours back so the recording stays small. The edge cases
below are that same answer edited in the one place they name, never an invented feed.

Without a container runtime the Bento cases skip; the shape cases do not.
"""

import json
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-helsinki-air-quality")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-air-quality-bento.yaml"
RECORDED = (Path(__file__).resolve().parent / "fixtures/helsinki/fmi-airquality-hourly.xml").read_text()
# The digest the pipeline runner pins (test_bystrica_pipelines.py, test_demo_feeds.py).
BENTO = "ghcr.io/warpstreamlabs/bento:1.22.0@sha256:ef674bebae45793c982c350d8f37c2c7661133c1dc65a5517fb977b36dc09841"
PREFIX = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:fmi-"

requires_docker = pytest.mark.skipif(
    shutil.which("docker") is None, reason="no container runtime, so Bento cannot run the mapping"
)


def run(document: str) -> list[dict]:
    """The mapping's processors over one fetched document, as the runner feeds it."""
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"],
        "output": {"stdout": {"codec": "all-bytes"}},
        "logger": {"level": "error"},
    }
    # The document goes in on stdin, so the configuration goes in by environment: JSON is YAML.
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
        pytest.skip("no container runtime")
    return {entity["id"]: entity for entity in run(RECORDED)}


@requires_docker
def test_every_station_of_the_feed_is_one_entity_with_its_newest_reading(recorded):
    assert len(recorded) == 11
    kallio = recorded[PREFIX + "kallio-2"]
    assert kallio == {
        "id": PREFIX + "kallio-2",
        "type": "AirQualityObserved",
        "name": {"type": "LanguageProperty", "languageMap": {"fi": "Helsinki Kallio 2"}},
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.9506, 60.18739]}},
        "dateObserved": {"type": "Property", "value": "2026-09-22T12:00:00Z"},
        # The 12:00 values of the recording, not the 09:00 or 10:00 ones before them.
        "pm10": {"type": "Property", "value": 7.2, "unitCode": "GQ", "observedAt": "2026-09-22T12:00:00Z"},
        "pm25": {"type": "Property", "value": 1.9, "unitCode": "GQ", "observedAt": "2026-09-22T12:00:00Z"},
        "airQualityIndex": {"type": "Property", "value": 1, "observedAt": "2026-09-22T12:00:00Z"},
        "districtCode": {"type": "Property", "value": "12"},
        "source": {"type": "Property", "value": "https://opendata.fmi.fi/wfs"},
    }


@requires_docker
def test_a_helsinki_station_names_its_district_and_a_neighbour_towns_station_names_none(recorded):
    """T-6467: the area join of a KPI meets rows on an attribute's value, never on geometry. Each
    code is the AdministrativeArea whose polygon holds the station's point (live, 2026-11-02)."""
    districts = {entity_id.removeprefix(PREFIX): entity["districtCode"]["value"]
                 for entity_id, entity in recorded.items() if "districtCode" in entity}
    assert districts == {"kallio-2": "12", "mannerheimintie": "02", "teollisuuskatu": "22",
                         "herttoniemi-2": "43", "vartiokyla-huivipolku": "45"}
    names = {entity_id: entity["name"]["languageMap"]["fi"] for entity_id, entity in recorded.items()}
    assert all(name.startswith("Helsinki") == ("districtCode" in recorded[entity_id])
               for entity_id, name in names.items())


@requires_docker
def test_every_entity_carries_source_so_a_steward_cannot_remove_it(recorded):
    """The air-quality App lets a steward delete only a record without `source` (`q: "!source"`)."""
    assert all(entity["source"]["value"] == "https://opendata.fmi.fi/wfs" for entity in recorded.values())


@requires_docker
def test_a_missing_newest_value_falls_back_to_the_hour_before():
    newest = re.escape("<BsWfs:Time>2026-09-22T12:00:00Z</BsWfs:Time>")
    # Kallio's 12:00 PM10 is the first 12:00 element of the recording; FMI writes a gap as NaN.
    edited = re.sub(
        newest + r"(\s*<BsWfs:ParameterName>PM10_PT1H_avg</BsWfs:ParameterName>\s*<BsWfs:ParameterValue>)7\.2<",
        lambda m: m.group(0).replace("7.2<", "NaN<"),
        RECORDED,
        count=1,
    )
    assert edited != RECORDED
    kallio = {e["id"]: e for e in run(edited)}[PREFIX + "kallio-2"]
    assert kallio["pm10"]["observedAt"] == "2026-09-22T11:00:00Z"
    assert kallio["pm25"]["observedAt"] == "2026-09-22T12:00:00Z"


@requires_docker
def test_an_empty_answer_writes_nothing():
    empty = re.sub(r"<wfs:member>.*</wfs:member>", "", RECORDED, flags=re.S)
    assert "<wfs:member>" not in empty
    assert run(empty) == []


@requires_docker
def test_a_single_member_answer_is_one_station_not_an_error():
    first, rest = RECORDED.split("</wfs:member>", 1)
    single = first + "</wfs:member>" + re.sub(r"<wfs:member>.*</wfs:member>", "", rest, flags=re.S)
    produced = run(single)
    assert [e["id"] for e in produced] == [PREFIX + "kallio-2"]
    assert set(produced[0]) == {"id", "type", "name", "location", "dateObserved", "pm10", "districtCode", "source"}


def test_the_pipeline_datasource_and_class_are_seeded_together():
    datasource = yaml.safe_load((HELSINKI / "helsinki-datasource-air-quality.yaml").read_text())
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-air-quality.yaml").read_text())
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"]
    assert datasource["spec"]["http"]["url"].startswith("https://opendata.fmi.fi/wfs?")
    assert "urban::observations::airquality::hourly::simple" in datasource["spec"]["http"]["url"]
    assert "AirQualityObserved" in model["spec"]["classes"]
    for name in ("helsinki-datasource-air-quality.yaml", "helsinki-pipeline-air-quality.yaml",
                 "helsinki-pipeline-air-quality-bento.yaml"):
        assert name in index, f"{name} is not seeded"


def test_the_model_declares_every_attribute_the_mapping_and_the_app_use():
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())
    defs = schema.get("$defs") or schema["definitions"]
    properties = set(defs["AirQualityObserved"]["properties"])
    # The mapping's attributes, plus the steward's note the air-quality App writes.
    for attr in ("name", "location", "dateObserved", "pm10", "pm25", "airQualityIndex", "districtCode", "source",
                 "stewardNote"):
        assert attr in properties, attr


def test_the_public_policy_admits_every_station_by_a_pattern_the_broker_pages_on():
    """Antares hands a policy idPattern to its store only inside an exact subset that has no `.`
    wildcard (antares-store id_pattern_sql); `fmi-.+` left every temporal read of the type walking
    the whole space into the 10 000-row ceiling, 403 TooManyResults (T-6052)."""
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-air-quality.yaml").read_text())
    (rule,) = policy["spec"]["information"]
    (selector,) = rule["entities"]
    pattern = selector["idPattern"]
    assert "." not in re.sub(r"\[[^\]]*\]", "", pattern), pattern
    stations = re.findall(r'"id": "([^"]+)", "name"', MAPPING.read_text())
    assert len(stations) == 11, stations
    for station in stations:
        assert re.fullmatch(pattern, PREFIX + station), station
    assert not re.fullmatch(pattern, "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:swarco-1-2")
