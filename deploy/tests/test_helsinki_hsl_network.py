"""HSL's network registers in the Helsinki spaces (T-5254, PL-03): fare zones, ticket sales points,
City Bike Vantaa's stations and the boardings per stop.

Bento runs each committed mapping over a recorded answer of the URL its DataSource declares (read on
2026-10-22, trimmed to a few records, `fixtures/helsinki/open-data/hsl-*.json`), to which the
fixture adds the bad records the owner's plausibility rule names (2026-10-22T18:10Z): a record
without a position, one outside the area (a swapped pair, a 0,0), one with a value out of range,
one with an unknown code and a duplicate. Each is dropped and counted by its reason, every written
entity is valid against the space's generated JSON Schema (what the gateway enforces) and against
the pinned Smart Data Model, and an answer with no record fails the run instead of writing nothing.
"""

import json
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-hsl-network")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
FIXTURES = Path(__file__).resolve().parent / "fixtures/helsinki/open-data"

# pipeline -> (DataSource, space, type, Smart Data Model, the ids it writes, the drops by reason)
FEEDS = {
    "fare-zones": ("hsl-fare-zones", "helsinki", "AdministrativeArea", "extensions/AdministrativeArea",
                   {"hsl-zone-A", "hsl-zone-B"},
                   {"noLocation": 1, "outOfArea": 1, "unknownCode": 1, "duplicate": 1}),
    "sales-points": ("hsl-sales-points", "helsinki", "PointOfInterest", "dataModel.PointOfInterest/PointOfInterest",
                     {"hsl-sales-1", "hsl-sales-61", "hsl-sales-157", "hsl-sales-599", "hsl-sales-632"},
                     {"noLocation": 1, "outOfArea": 2, "unknownCode": 1, "notInUse": 1, "noId": 0, "noName": 0,
                      "duplicate": 1}),
    "vantaa-bikes": ("hsl-vantaa-bikes", "helsinki", "BikeHireDockingStation",
                     "dataModel.Transportation/BikeHireDockingStation",
                     {"vantaa-1001", "vantaa-1003", "vantaa-1005", "vantaa-1007", "vantaa-1009"},
                     {"noLocation": 1, "outOfArea": 1, "outOfRange": 1, "noId": 0, "noName": 0, "duplicate": 1}),
    "stop-boardings": ("hsl-stop-boardings", "helsinki-kpi", "KeyPerformanceIndicator", None,
                       {"hsl-boardings-helsingin-rautatieasema-1", "hsl-boardings-pasilan-asema-2",
                        "hsl-boardings-ilmalan-asema-3", "hsl-boardings-huopalahden-asema-4",
                        "hsl-boardings-valimon-asema-5", "hsl-boardings-elielinaukio-389",
                        "hsl-boardings-elielinaukio-390"},
                       {"noLocation": 1, "outOfArea": 1, "outOfRange": 1, "noName": 1, "duplicate": 1}),
}


def mapping(pipeline: str) -> Path:
    return HELSINKI / f"helsinki-pipeline-{pipeline}-bento.yaml"


def document(pipeline: str) -> bytes:
    return (FIXTURES / f"{FEEDS[pipeline][0]}.json").read_bytes()


def schema(space: str) -> Path:
    return HELSINKI / f"{space}.v1.schema.json"


@pytest.fixture(scope="module")
def written() -> dict[str, list[dict]]:
    return {p: open_data.run(mapping(p), document(p), FEEDS[p][1]) for p in FEEDS}


@pytest.fixture(scope="module")
def dropped(tmp_path_factory) -> dict[str, dict]:
    """The counts each mapping logs: the same processors without the WARN line and the last step,
    which keeps only the entities."""
    found = {}
    for p in FEEDS:
        config = yaml.safe_load(mapping(p).read_text())
        steps = config["pipeline"]["processors"][0]["try"]
        assert [list(step)[0] for step in steps[-2:]] == ["switch", "mapping"]
        config["pipeline"]["processors"] = [{"try": steps[:-2]}]
        path = tmp_path_factory.mktemp(p) / "bento.yaml"
        path.write_text(yaml.safe_dump(config))
        found[p] = open_data.run(path, document(p), FEEDS[p][1])["dropped"]
    return found


def test_every_pipeline_reads_its_own_datasource_and_writes_its_type_through_the_spaces_endpoint():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for pipeline, (source, space, kind, *_rest) in FEEDS.items():
        manifest = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{pipeline}.yaml").read_text())
        spec = manifest["spec"]
        assert spec["source"]["dataSourceRef"]["name"] == source
        assert spec["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:{{orgDomain}}:{space}:{space if space != 'helsinki' else 'helsinki-all'}"
        assert spec["output"] == {"type": kind, "mode": "upsert"}, "upsert: a run never deletes what it did not read"
        assert spec["period"] == "24h" and spec["schedule"]
        datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{pipeline}.yaml").read_text())
        assert datasource["metadata"]["name"] == source
        # The one host these registers come from, over TLS, with a bounded wait.
        assert datasource["spec"]["http"]["url"].startswith("https://hub.arcgis.com/api/v3/datasets/")
        assert datasource["spec"]["http"]["timeout"] == "60s"
        assert index[f"helsinki-pipeline-{pipeline}.yaml"] == f"projects/helsinki/pipelines/{pipeline}/pipeline.yaml"
        assert index[f"helsinki-pipeline-{pipeline}-bento.yaml"] == f"projects/helsinki/pipelines/{pipeline}/bento.yaml"
        assert index[f"helsinki-datasource-{pipeline}.yaml"] == f"projects/helsinki/datasources/{source}.yaml"


@requires_docker
@pytest.mark.parametrize("pipeline", FEEDS)
def test_the_real_records_are_written_and_each_bad_one_is_dropped(written, pipeline):
    _source, space, kind, _model, ids, _drops = FEEDS[pipeline]
    entities = written[pipeline]
    prefix = f"urn:ngsi-ld:{kind}:hel.fi:{space}:"
    assert {e["id"].removeprefix(prefix) for e in entities} == ids
    assert all(e["type"] == kind and e["id"].startswith(prefix) for e in entities)


@requires_docker
@pytest.mark.parametrize("pipeline", FEEDS)
def test_each_dropped_record_is_counted_by_its_reason(dropped, pipeline):
    assert dropped[pipeline] == FEEDS[pipeline][5]


@requires_docker
@pytest.mark.parametrize("pipeline", FEEDS)
def test_every_entity_is_valid_against_the_space_schema_and_the_smart_data_model(written, pipeline):
    _source, space, _kind, model, _ids, _drops = FEEDS[pipeline]
    for entity in written[pipeline]:
        assert open_data.schema_errors(schema(space), entity) == [], entity["id"]
        if model:
            assert open_data.sdm_errors(model, entity) == [], entity["id"]


@requires_docker
@pytest.mark.parametrize("pipeline", FEEDS)
def test_every_position_lies_in_the_region_with_longitude_first(written, pipeline):
    for entity in written[pipeline]:
        geometry = entity["location"]["value"]
        points = {"Point": lambda c: [c], "Polygon": lambda c: [p for r in c for p in r]}[geometry["type"]](geometry["coordinates"])
        # Finland: longitude 19 to 32, latitude 59 to 71; a swapped pair falls outside both.
        assert all(19 < lon < 32 and 59 < lat < 71 for lon, lat in points), entity["id"]


@requires_docker
@pytest.mark.parametrize("pipeline", FEEDS)
def test_an_answer_without_a_record_fails_the_run(pipeline):
    """The runner fails a run whose message is errored; the `try` keeps every later step from
    turning the refused answer into an empty write."""
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": yaml.safe_load(mapping(pipeline).read_text())["pipeline"],
        "output": {"stdout": {"codec": "all-bytes"}},
        "logger": {"level": "error"},
    }
    empty = json.dumps({"type": "FeatureCollection", "features": []})
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", f"JC_SPACE={FEEDS[pipeline][1]}",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", open_data.BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=empty.encode(), capture_output=True, timeout=120,
    )
    assert b"holds no" in result.stderr and b"the run fails" in result.stderr, result.stderr
    # Nothing was mapped: the message is the refused answer, which no endpoint takes as entities.
    assert json.loads(result.stdout) == json.loads(empty)


@requires_docker
def test_a_sales_point_carries_its_kind_zone_and_note_per_language(written):
    machine = next(e for e in written["sales-points"] if e["id"].endswith(":hsl-sales-1"))
    assert machine["serviceCategory"]["value"] == "ticketMachine"
    assert machine["fareZone"]["value"] == "A"
    assert machine["description"]["languageMap"] == {"fi": "Maksutapana vain maksukortit", "sv": "Betalning med betalkort",
                                                     "en": "Payment by card"}
    assert {e["serviceCategory"]["value"] for e in written["sales-points"]} == {
        "ticketMachine", "ticketSalesPoint", "ticketParkingMeter", "ticketServicePoint"}


@requires_docker
def test_a_vantaa_station_has_its_slots_and_no_live_count(written):
    station = next(e for e in written["vantaa-bikes"] if e["id"].endswith(":vantaa-1001"))
    assert station["totalSlotNumber"]["value"] == 20
    assert station["address"]["value"] == {"streetAddress": "Tikkurilantie 30", "addressLocality": "Vantaa", "addressCountry": "FI"}
    assert "availableBikeNumber" not in station and "freeSlotNumber" not in station


@requires_docker
def test_a_fare_zone_is_an_area_of_its_own_division_level(written):
    zone = next(e for e in written["fare-zones"] if e["id"].endswith(":hsl-zone-A"))
    assert zone["divisionLevel"]["value"] == "fareZone" and zone["fareZone"]["value"] == "A"
    assert zone["name"]["languageMap"]["fi"] == "HSL-vyöhyke A"


@requires_docker
def test_a_boarding_count_keeps_its_window_and_each_platform(written):
    station = next(e for e in written["stop-boardings"] if e["id"].endswith(":hsl-boardings-helsingin-rautatieasema-1"))
    assert station["currentValue"] == {"type": "Property", "value": 56005}
    assert station["calculationPeriod"]["value"] == {"start": "2016-11-01T00:00:00Z", "end": "2016-11-30T23:59:59Z"}
    assert station["name"]["value"] == "hsl-boardings-helsingin-rautatieasema-1"
    # Two platforms of one name a few metres apart are two counts, not a duplicate.
    platforms = [e["currentValue"]["value"] for e in written["stop-boardings"] if "elielinaukio" in e["id"]]
    assert sorted(platforms) == [1014, 1153]
    assert isinstance(station["updatedAt"]["value"], str) and station["updatedAt"]["value"].endswith("Z")


@requires_docker
def test_a_city_bike_station_message_maps_to_one_docking_station():
    """hsl-citybikes-gbfs-info splits station_information's data.stations (records, PL-114), so
    the citybikes mapping gets one GBFS station per message (T-6576), not the whole answer."""
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-bikes-info.yaml").read_text())
    assert source["spec"]["http"]["records"] == "data.stations"
    station = {"station_id": "001", "name": "Kaivopuisto", "lat": 60.155411, "lon": 24.950391,
               "capacity": 30}
    [entity] = open_data.run_all(mapping("bikes-info"), json.dumps(station).encode(), "helsinki")
    assert entity["id"] == "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001"
    assert entity["name"]["languageMap"] == {"fi": "Kaivopuisto"}
    assert entity["location"]["value"]["coordinates"] == [24.950391, 60.155411]
