"""Barcelona open data, environment rest (T-5413, PL-03, DM-01, DM-61): Meteocat's forecast for Barcelonès.

Bento runs the committed mapping over `fixtures/barcelona/prediccio-barcelones.xml`: Open Data BCN's
prediccio-meteorologica-del-barcelones file as its download URL answered it at 2026-10-24T17:22Z
(datacreacio 24-10-2026 15:00:00), with its symbol and class tables and the forecasts of two of
its 43 comarques, Barcelonès (13) and Baix Llobregat (11). Every entity is checked against the
space's generated JSON Schema, which the gateway enforces, and against the Smart Data Model at its
vendored pin. The edge cases are that recording edited in the one place each names. Without a
container runtime the Bento cases skip; the seed cases do not.
"""

import shutil
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_barcelona_environment import first_step

pytestmark = pytest.mark.xdist_group("docker-barcelona-environment")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = CITY / "barcelona-pipeline-weather-forecast-bento.yaml"
RECORDED = (Path(__file__).resolve().parent / "fixtures/barcelona/prediccio-barcelones.xml").read_text()
PREFIX = "urn:ngsi-ld:WeatherForecast:barcelona.cat:barcelona:barcelones-day-"
AS_OF = {"bcn_now": "2026-10-24T17:22:00Z"}
DAY_1 = '<variable data="25-10-2026" dia="1"'
TEMPS_1 = 'tempmax="22" tempmin="15"'


def run(document: str) -> list[dict]:
    return open_data.run_all(MAPPING, document.encode(), "barcelona", "barcelona.cat", AS_OF)


def barcelones(document: str) -> str:
    """The Barcelonès block of a document, to edit one day without touching Baix Llobregat."""
    start = document.index('<prediccio idcomarca="13">')
    return document[start:document.index("</prediccio>", start)]


def edited(old: str, new: str) -> str:
    block = barcelones(RECORDED)
    assert old in block, old
    return RECORDED.replace(block, block.replace(old, new, 1))


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in run(RECORDED)}


@requires_docker
def test_each_day_of_the_barcelones_forecast_is_one_weather_forecast(written):
    assert sorted(written) == [PREFIX + "1", PREFIX + "2"]
    day = written[PREFIX + "1"]
    assert day["name"]["value"] == "Predicció del Barcelonès, dia 1"
    # 15:00 in Madrid summer time is 13:00 UTC.
    assert day["dateIssued"]["value"] == "2026-10-24T13:00:00Z"
    # 25 October 2026 is the night summer time ends: the day starts at +02:00 and ends at +01:00.
    assert day["validFrom"]["value"] == "2026-10-24T22:00:00Z"
    assert day["validTo"]["value"] == "2026-10-25T23:00:00Z"
    assert day["dayMaximum"]["value"] == {"temperature": 22}
    assert day["dayMinimum"]["value"] == {"temperature": 15}
    assert day["weatherType"]["value"] == "sol i núvols alts (matí), sol (tarda)"
    assert day["areaServed"]["value"] == "Barcelonès"
    assert written[PREFIX + "2"]["validFrom"]["value"] == "2026-10-25T23:00:00Z"
    # Observed at the issue, so a rerun over the same file adds no history row (temporal audit: duplicates-rows).
    for attr in ("dateIssued", "validFrom", "validTo", "dayMaximum", "dayMinimum", "weatherType"):
        assert day[attr]["observedAt"] == "2026-10-24T13:00:00Z", attr


@requires_docker
def test_every_day_is_one_the_gateway_accepts_and_valid_in_its_smart_data_model(written):
    bbox = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]
    for entity in written.values():
        assert open_data.schema_errors(CITY / "barcelona.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.Weather/WeatherForecast", entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert bbox[0] <= lon <= bbox[2] and bbox[1] <= lat <= bbox[3], entity["id"]


@requires_docker
@pytest.mark.parametrize("old, new, reason", [
    (TEMPS_1, 'tempmax="22" tempmin="n/a"', "notANumber"),
    (TEMPS_1, 'tempmax="62" tempmin="15"', "outOfRange"),
    (TEMPS_1, 'tempmax="12" tempmin="15"', "outOfRange"),
    (DAY_1, '<variable data="25-13-2026" dia="1"', "notADay"),
    (DAY_1, '<variable data="20-10-2026" dia="1"', "outsideTheForecast"),
    ('<variable data="26-10-2026" dia="2"', '<variable data="26-10-2026" dia="1"', "duplicateDay"),
], ids=["no-number", "too-hot", "min-above-max", "bad-date", "ended", "same-day"])
def test_an_unreadable_an_out_of_range_a_stale_and_a_duplicate_day_are_dropped_and_counted(old, new, reason):
    answer = first_step(edited(old, new), AS_OF, MAPPING)
    assert answer["dropped"][reason] == 1, answer["dropped"]
    assert sum(answer["dropped"].values()) == 1
    assert len(answer["entities"]) == 1


@requires_docker
@pytest.mark.parametrize("document, error", [
    (RECORDED.replace('<prediccio idcomarca="13">', '<prediccio idcomarca="99">'), "no valid day for Barcelonès"),
    (RECORDED.replace('datacreacio="24-10-2026 15:00:00"', 'datacreacio="20-10-2026 15:00:00"'), "more than two days ago"),
    (RECORDED.replace('datacreacio="24-10-2026 15:00:00"', 'datacreacio=""'), "no issue time"),
    ("<html><body>Service unavailable</body></html>", "no Meteocat XML"),
    (RECORDED + " " * 1048576, "more than the 1 MiB read"),
], ids=["no-barcelones", "stale", "no-issue-time", "html", "oversized"])
def test_a_file_without_a_current_barcelones_forecast_fails_the_run(document, error):
    assert error in first_step(document, AS_OF, MAPPING)["error"]


def test_the_forecast_replaces_its_own_type_from_the_city_and_is_public_with_its_history():
    pipeline = yaml.safe_load((CITY / "barcelona-pipeline-weather-forecast.yaml").read_text())["spec"]
    source = yaml.safe_load((CITY / "barcelona-datasource-weather-forecast.yaml").read_text())["spec"]["http"]
    endpoint = yaml.safe_load((CITY / "barcelona-endpoint-weather-forecast.yaml").read_text())["spec"]
    policy = yaml.safe_load((CITY / "barcelona-policy-public-weather-forecast.yaml").read_text())["spec"]
    space = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]
    assert source["url"].startswith("https://opendata-ajuntament.barcelona.cat/")
    assert pipeline["source"]["dataSourceRef"]["name"] == "bcn-weather-forecast"
    assert pipeline["output"] == {"type": "WeatherForecast", "mode": "replace"}
    assert endpoint["policyRef"].endswith(":public-weather-forecast")
    assert endpoint["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "weather-forecast"}
    assert endpoint["publish"]["ckan"]["instanceRef"]["name"] == "barcelona"
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert not {"upsertBatch", "createBatch", "deleteBatch", "createEntity", "deleteEntity"} & set(policy["operations"])
    assert policy["information"][0]["entities"] == [{"type": "WeatherForecast"}]
    [kept] = [r for r in space["retention"] if r["entityType"] == "WeatherForecast"]
    assert kept["history"] == "P30D"
    assert "WeatherForecast" in {r["entityType"] for r in space["quality"]}
