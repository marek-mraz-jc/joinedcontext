"""Prague's environment layers (T-5429, PL-03): the REZZO 1 and REZZO 2 air pollution sources as
EmissionSource, the radon risk map as RadonRiskArea, the designated flood areas as FloodZone, the
flood defence lines as FloodDefence (IPR Praha geoportal), and ČHMÚ's eight Prague weather stations
as WeatherObserved; the datasets praha-zdroje-znecisteni, praha-radon, praha-zaplavova-uzemi,
praha-protipovodnova-ochrana and praha-pocasi.

Bento runs each committed mapping over real records downloaded on 2026-10-24
(`fixtures/praha/environment.json`: per IPR layer its smallest records, one per code where the
layer has a code list, without the REZZO operator fields the mapping never reads; ČHMÚ's day file
of station 11518, its last hour, and the directory listing). The edge cases are that answer edited
in the one place they name. Each entity is checked against the space's generated JSON Schema, which
the gateway enforces, and the Smart Data Model or extension schema of its type.
"""

import copy
import json
import re
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-environment")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/environment.json").read_text())
SPACE = "praha-mesto"
LAYERS = {
    "rezzo1": ("d2382afbf9454951ab71a0a89c762d15", "EmissionSource"),
    "rezzo2": ("8f600add6b104be9810b23c1bf212b78", "EmissionSource"),
    "radon": ("a74bc85ba5354490a23afcfe495693d9", "RadonRiskArea"),
    "flood-q5": ("c4ef1880cd5d4e62896f8cb845217881", "FloodZone"),
    "flood-q20": ("fc295e699c5346de9cf6a7e1e634726b", "FloodZone"),
    "flood-q50": ("040756d41d294971ad730304b792cb03", "FloodZone"),
    "flood-q100": ("c9dc5fa395b2478a8db0a7cc5be0e447", "FloodZone"),
    "flood-vltava": ("7a4bf2d99de44645ac5768f65d8a9692", "FloodZone"),
    "flood-defences": ("1a09887626a243aeab7910d3ab2477d2", "FloodDefence"),
    "flood-sandbags": ("98a6eba14f8646e1a6b7e3cd805f484b", "FloodDefence"),
}
RECORDED_LAYERS = [name for name in LAYERS if name in RECORDED]
ERRORED = {"mapping": 'root = if errored() { {"__error": error()} } else { this }'}


def bento(pipeline: dict, document: bytes) -> tuple[list[dict], list[str]]:
    """The processors over one answer: the entities written and the errors of the messages the
    runner refuses (an errored message fails the run, nothing is written)."""
    pipeline = copy.deepcopy(pipeline)
    pipeline["pipeline"]["processors"].append(ERRORED)
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run_all(pinned, document, SPACE, "praha.eu")
    errors = [m["__error"] for m in answer if "__error" in m]
    return ([] if errors else answer), errors


def mapping(name: str) -> dict:
    return yaml.safe_load((PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text())


def outcome(name: str, document: dict) -> tuple[list[dict], list[str]]:
    return bento(mapping(f"geo-{name}"), json.dumps(document).encode())


def run(name: str, document: dict | None = None) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document)
    assert not errors, errors
    return {e["id"].rsplit(f":geo-{name}-", 1)[1]: e for e in entities}


def guid(feature: dict) -> str:
    return feature["properties"]["globalid"].strip("{}").lower()


def edited(name: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[name])
    edit(document["features"])
    return document


def sdm(entity: dict) -> list[str]:
    model = "dataModel.Weather/WeatherObserved" if entity["type"] == "WeatherObserved" else f"extensions/{entity['type']}"
    return open_data.sdm_errors(model, entity)


@pytest.fixture(scope="module")
def written():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in RECORDED_LAYERS}


@requires_docker
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    for name in RECORDED_LAYERS:
        item, kind = LAYERS[name]
        assert sorted(written[name]) == sorted(guid(f) for f in RECORDED[name]["features"]), name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:geo-{name}-")
            assert entity["type"] == kind
            assert entity["source"]["value"] == f"https://geoportalpraha.cz/data-a-sluzby/{item}"
            assert entity["dataProvider"]["value"] == "datový podklad © IPR Praha (CC BY 4.0)"
            assert entity["name"]["languageMap"]["cs"]
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not sdm(entity), (entity["id"], sdm(entity))


@requires_docker
def test_an_emission_source_carries_its_register_fuel_emissions_and_address_but_no_operator(written):
    feature = RECORDED["rezzo1"]["features"][0]
    source = written["rezzo1"][guid(feature)]
    p = feature["properties"]
    assert source["registerCategory"]["value"] == "rezzo1"
    assert source["fuelType"]["value"] == p["division"]
    assert source["noxEmission"]["value"] == p["nox"] and source["pm10Emission"]["value"] == p["pm10"]
    assert source["address"]["value"] == {"streetAddress": "U továren 770/1b", "addressLocality": "Praha 15", "addressCountry": "CZ"}
    assert source["name"]["languageMap"] == {"cs": "Zdroj REZZO 1: U továren 770/1b (Praha 15)"}
    assert written["rezzo2"][guid(RECORDED["rezzo2"]["features"][0])]["registerCategory"]["value"] == "rezzo2"
    # The operator, the source's name and the company number are never read, whatever they hold.
    document = copy.deepcopy(RECORDED["rezzo2"])
    document["features"][0]["properties"].update({"provoz": "Jan Novák", "zdroj": "Jan Novák, kotelna", "ico": "12345678"})
    entity = run("rezzo2", document)[guid(document["features"][0])]
    assert not {"Novák", "12345678"} & set(re.findall(r"[\w.@]+", json.dumps(entity, ensure_ascii=False)))


@requires_docker
def test_an_emission_out_of_range_is_left_out_and_the_rest_of_the_source_stays():
    def edit(features):
        features[0]["properties"].update({"nox": -1.0, "co": 250000.0})
    sources = run("rezzo1", edited("rezzo1", edit))
    source = sources[guid(RECORDED["rezzo1"]["features"][0])]
    assert "noxEmission" not in source and "coEmission" not in source
    assert source["so2Emission"]["value"] == RECORDED["rezzo1"]["features"][0]["properties"]["so2"]
    assert len(sources) == 3


@requires_docker
def test_the_code_lists_become_the_model_values_and_name_the_area(written):
    radon = {e["radonRiskCategory"]["value"]: e["name"]["languageMap"]["cs"] for e in written["radon"].values()}
    assert radon == {"transitional": "Radonové riziko: přechodné", "low": "Radonové riziko: nízké",
                     "medium": "Radonové riziko: střední", "high": "Radonové riziko: vysoké"}
    zones = {e["floodZoneCategory"]["value"] for e in written["flood-vltava"].values()}
    assert zones == {"active", "flowThrough", "nonFlowThrough", "protectedByCity", "protectedIndividually"}
    (q100,) = written["flood-q100"].values()
    assert q100["floodReturnPeriod"]["value"] == 100 and q100["name"]["languageMap"] == {"cs": "Záplavové území Q100"}
    assert "floodZoneCategory" not in q100
    lines = {(e["defenceType"]["value"], e["designFlood"]["value"], e["protectedBy"]["value"]) for e in written["flood-defences"].values()}
    assert {t for t, _, _ in lines} >= {"fixed", "mobileWall"} and {q for _, q, _ in lines} == {"q100", "flood2002"}
    assert {e["defenceType"]["value"] for e in written["flood-sandbags"].values()} == {"sandbags"}


@requires_docker
def test_outlines_are_rounded_to_6_decimals_without_a_height(written):
    def edit(features):
        ring = features[0]["geometry"]["coordinates"][0][0]
        ring[0] = ring[0][:2] + [235.5]
    areas = run("radon", edited("radon", edit))
    for entity in [*areas.values(), *written["flood-vltava"].values(), *written["flood-defences"].values()]:
        text = json.dumps(entity["location"]["value"]["coordinates"])
        assert not re.search(r"\.\d{7,}", text), entity["id"]
        assert "235.5" not in text


@requires_docker
def test_an_unknown_code_an_area_outside_prague_and_a_duplicate_are_dropped():
    """The owner's plausibility rule (2026-10-22T18:10Z): a code outside the layer's list, an outline
    reaching outside Prague's box and a second record of one GUID are dropped and counted; the
    other areas stay."""
    def edit(features):
        features[0]["properties"]["rn_index"] = 7
        features[1]["geometry"]["coordinates"][0][0][0] = [16.6, 49.2]
        features.append(copy.deepcopy(features[2]))
        for n in (5, 6):
            more = copy.deepcopy(features[3])
            more["properties"]["globalid"] = "{00000000-0000-4000-8000-00000000000%d}" % n
            features.append(more)
    areas = run("radon", edited("radon", edit))
    first, second, third, fourth = (guid(f) for f in RECORDED["radon"]["features"])
    # Seven read: the unknown code, the outline outside Prague and the duplicate dropped.
    assert sorted(areas) == sorted([third, fourth, "00000000-0000-4000-8000-000000000005", "00000000-0000-4000-8000-000000000006"])


@requires_docker
def test_a_layer_where_fewer_than_half_survive_fails_the_run():
    def edit(features):
        for f in features:
            f["geometry"] = {"type": "Point", "coordinates": [14.4, 50.08]}
    entities, errors = outcome("flood-vltava", edited("flood-vltava", edit))
    assert entities == [] and len(errors) == 1 and "only 0 of 5 records" in errors[0], errors


@requires_docker
@pytest.mark.parametrize("name", ["radon", "rezzo1", "flood-defences"])
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no record"),
    ({"error": {"code": 500, "message": "Internal error"}}, "did not answer a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_layer(name, document, reason):
    entities, errors = outcome(name, document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


def test_each_layer_is_read_weekly_from_its_ipr_download_into_the_city_space():
    for name, (item, kind) in LAYERS.items():
        source = yaml.safe_load((PRAHA / f"praha-datasource-geo-{name}.yaml").read_text())
        assert source["spec"]["http"]["url"] == (
            f"https://hub.arcgis.com/api/v3/datasets/{item}_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1"
        )
        assert "authorization" not in source["spec"]["http"]
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-geo-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"ipr-geo-{name}"
        assert pipeline["spec"]["period"] == "168h"
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")
        text = (PRAHA / f"praha-pipeline-geo-{name}-bento.yaml").read_text()
        assert f'let item = "{item}"' in text and "16777216" in text
        # The radon map and the Q layers are 2.5 to 8.4 MB: 256 MiB holds them (128 MiB did not, 2026-10-24).
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] == (256 if name == "radon" or re.fullmatch(r"flood-q\d+", name) else 128)


# --- ČHMÚ weather ---

def today_listing() -> str:
    now = datetime.now(timezone.utc)
    return RECORDED["chmi-listing"].replace("20261024", now.strftime("%Y%m%d")).replace("20261023", (now - timedelta(days=1)).strftime("%Y%m%d"))


def weather(values_by_station: dict[str, list]) -> tuple[list[dict], list[str]]:
    """The weather mapping over today's listing, its branch (one http call per station) replaced
    by the recorded answer of each station, or none."""
    pipeline = mapping("chmi-weather")
    steps = pipeline["pipeline"]["processors"][0]["try"]
    (at,) = [i for i, s in enumerate(steps) if "branch" in s]
    steps[at] = {"mapping": "root = this.assign({ \"values\": %s.get(this.wsi).or([]) })" % json.dumps(values_by_station)}
    return bento(pipeline, today_listing().encode())


def recent(minutes: int = 0) -> list:
    """Station 11518's recorded last hour, moved so its newest value is `minutes` old."""
    values = RECORDED["chmi-11518"]["data"]["data"]["values"]
    newest = max(datetime.strptime(v[2], "%Y-%m-%dT%H:%M:%SZ") for v in values)
    shift = datetime.now(timezone.utc).replace(tzinfo=None, second=0, microsecond=0) - timedelta(minutes=minutes) - newest
    return [[v[0], v[1], (datetime.strptime(v[2], "%Y-%m-%dT%H:%M:%SZ") + shift).strftime("%Y-%m-%dT%H:%M:%SZ"), *v[3:]] for v in values]


def last(values: list, element: str) -> float:
    return max((v for v in values if v[1] == element), key=lambda v: v[2])[3]


@requires_docker
def test_a_station_becomes_one_weather_observation_with_its_newest_values():
    values = recent(5)
    entities, errors = weather({"0-20000-0-11518": values})
    assert not errors, errors
    (station,) = entities
    assert station["id"] == f"urn:ngsi-ld:WeatherObserved:praha.eu:{SPACE}:chmi-0-20000-0-11518"
    assert station["name"]["languageMap"] == {"cs": "Praha, Ruzyně"}
    assert station["location"]["value"] == {"type": "Point", "coordinates": [14.255556, 50.100278]}
    assert station["temperature"]["value"] == last(values, "T")
    assert station["relativeHumidity"]["value"] == round(last(values, "H") / 100, 3)
    assert station["atmosphericPressure"]["value"] == last(values, "P")
    assert station["windSpeed"]["value"] == last(values, "F") and station["gustSpeed"]["value"] == last(values, "Fmax")
    assert station["dateObserved"]["value"] == max(v[2] for v in values if v[1] == "T")
    # The catalogue's charts read each value's history by observedAt (T-5432): every reading carries
    # the time of its own value.
    for attr, element in {"temperature": "T", "relativeHumidity": "H", "atmosphericPressure": "P", "windSpeed": "F", "gustSpeed": "Fmax"}.items():
        assert station[attr]["observedAt"] == max(v[2] for v in values if v[1] == element), attr
    assert station["dataProvider"]["value"] == "Český hydrometeorologický ústav (CC BY 4.0)"
    assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", station)
    assert not sdm(station), sdm(station)


@requires_docker
def test_out_of_range_and_stale_values_are_left_out_and_a_silent_station_writes_nothing():
    values = recent(5)
    newest_t = max((v for v in values if v[1] == "T"), key=lambda v: v[2])
    newest_t[3] = 71.0  # out of range: the next older T is read
    stale = [[v[0].replace("11518", "11519"), *v[1:]] for v in recent(240)]
    entities, errors = weather({"0-20000-0-11518": values, "0-20000-0-11519": stale})
    assert not errors, errors
    (station,) = entities
    older = max((v for v in values if v[1] == "T" and v is not newest_t), key=lambda v: v[2])
    assert station["temperature"]["value"] == older[3]


def test_the_listing_names_the_files_and_a_station_missing_today_reads_yesterday():
    text = mapping("chmi-weather")["pipeline"]["processors"][0]["try"][1]["mapping"]
    assert 'now().ts_format("20060102", "UTC")' in text and "86400" in text
    assert len(re.findall(r'"wsi": "0-2', text)) == 8


@requires_docker
def test_a_listing_without_the_prague_stations_fails_the_run():
    pipeline = mapping("chmi-weather")
    entities, errors = bento(pipeline, b"<html><body><pre></pre></body></html>")
    assert entities == [] and len(errors) == 1 and "none of the Prague stations" in errors[0], errors


def test_the_weather_is_read_hourly_from_chmi_only():
    source = yaml.safe_load((PRAHA / "praha-datasource-chmi-weather.yaml").read_text())
    assert source["spec"]["http"]["url"] == "https://opendata.chmi.cz/meteorology/climate/now/data/"
    pipeline = yaml.safe_load((PRAHA / "praha-pipeline-chmi-weather.yaml").read_text())
    assert pipeline["spec"]["period"] == "1h" and pipeline["spec"]["output"] == {"type": "WeatherObserved", "mode": "upsert"}
    steps = mapping("chmi-weather")["pipeline"]["processors"][0]["try"]
    (branch,) = [s["branch"] for s in steps if "branch" in s]
    (call,) = [p["http"] for p in branch["processors"] if "http" in p]
    assert call["url"].startswith("https://opendata.chmi.cz/meteorology/climate/now/data/10m-")
    assert "1048576" in json.dumps(branch)


# --- datasets ---

def test_each_dataset_reads_only_its_own_types():
    expected = {
        "zdroje-znecisteni": ["EmissionSource"],
        "radon": ["RadonRiskArea"],
        "zaplavova-uzemi": ["FloodZone"],
        "hluk": ["NoiseZone"],
        "protipovodnova-ochrana": ["FloodDefence"],
        "pocasi": ["WeatherObserved"],
    }
    for name, types in expected.items():
        policy = yaml.safe_load((PRAHA / f"praha-policy-public-{name}.yaml").read_text())
        assert [e["type"] for e in policy["spec"]["information"][0]["entities"]] == types
        assert policy["spec"]["assignee"] == {"kind": "role", "id": "public"}
        assert not {"create", "update", "delete", "upsert"} & {op.lower() for op in policy["spec"]["operations"]}
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{name}.yaml").read_text())
        assert endpoint["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:praha-mesto:public-{name}"
        assert endpoint["spec"]["catalog"]["license"] == "CC_BY_4_0"
        assert endpoint["spec"]["catalog"]["personalData"] == "does-not-contain"
    zones = yaml.safe_load((PRAHA / "praha-endpoint-zaplavova-uzemi.yaml").read_text())
    assert len(zones["spec"]["catalog"]["source"]) == 6
