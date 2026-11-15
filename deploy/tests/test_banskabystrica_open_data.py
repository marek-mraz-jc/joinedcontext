"""The city's open data in `banskabystrica-verejne` (T-2781, PL-03, DM-01, DM-61, PF-84).

Bento runs each committed mapping over a recorded answer of the URL its DataSource declares,
fetched on 2026-09-25 and trimmed to a few records (`fixtures/banskabystrica/`), and every entity
it writes is checked against the space's generated JSON Schema, which the gateway enforces. The
EEA files are the publisher's own Parquet cut to the last 24 hours with the same encoding (INT96
times, decimal values); the one edited file marks the latest hour withdrawn. The city's further
ŠÚ SR cubes are in test_bystrica_pipelines.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import json
import re
import shutil
import tempfile
import urllib.parse
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-banskabystrica-open-data")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/banskabystrica"
FIXTURES = Path(__file__).resolve().parent / "fixtures/banskabystrica"
SPACE = "banskabystrica-verejne"
DOMAIN = "banskabystrica.sk"

# pipeline -> (fixture, the type it writes, how many entities the recording holds)
FEEDS = {
    "podujatia": ("podujatia.json", "Event", 5),
    # Six rows: five in the city, one in Považská Bystrica.
    "skoly": ("skoly.csv", "Organization", 5),
    "ovzdusie-pm10": ("eea-sk0263a-pm10.parquet", "AirQualityObserved", 1),
    "ovzdusie-pm25": ("eea-sk0263a-pm25.parquet", "AirQualityObserved", 1),
    # The station's last 24 hours of 2026-10-15 (T-4153); every O3 hour there is validity 4.
    "ovzdusie-no2": ("eea-sk0263a-no2.parquet", "AirQualityObserved", 1),
    "ovzdusie-o3": ("eea-sk0263a-o3.parquet", "AirQualityObserved", 1),
}


def recorded(name: str) -> bytes:
    return (FIXTURES / name).read_bytes()


def run(pipeline: str, document: bytes) -> list[dict]:
    return open_data.run(CITY / f"pipeline-{pipeline}-bento.yaml", document, SPACE, DOMAIN)


def schema_errors(entity: dict) -> list[str]:
    return open_data.schema_errors(CITY / "banskabystrica-verejne.v1.schema.json", entity)


def events_edited(edit) -> bytes:
    document = json.loads(recorded("podujatia.json"))
    edit(document)
    return json.dumps(document, ensure_ascii=False).encode()


def manifest(name: str) -> dict:
    return yaml.safe_load((CITY / name).read_text())


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name, recorded(fixture)) for name, (fixture, _, _) in FEEDS.items()}


@requires_docker
def test_every_feed_writes_its_one_type_and_every_record_it_can(written):
    for name, (_, kind, count) in FEEDS.items():
        assert {e["type"] for e in written[name]} == {kind}, name
        assert len(written[name]) == count, name


@requires_docker
def test_every_entity_is_one_the_gateway_accepts(written):
    for name, entities in written.items():
        for entity in entities:
            assert not schema_errors(entity), (name, entity["id"], schema_errors(entity))


# The FIWARE Smart Data Model each written type is (T-4329), validated at its vendored pin.
SDM = {"Event": "dataModel.TourismDestinations/Event", "Organization": "dataModel.Organization/Organization",
       "AirQualityObserved": "dataModel.Environment/AirQualityObserved"}


@requires_docker
def test_every_entity_is_valid_in_its_smart_data_model(written):
    for name, entities in written.items():
        for entity in entities:
            assert open_data.sdm_errors(SDM[entity["type"]], entity) == [], (name, entity["id"])


@requires_docker
def test_every_id_is_the_four_segment_urn_of_the_space_and_unique(written):
    for name, entities in written.items():
        ids = [e["id"] for e in entities]
        assert len(ids) == len(set(ids)), name
        for entity in entities:
            assert re.fullmatch(rf"urn:ngsi-ld:{entity['type']}:banskabystrica\.sk:{SPACE}:[A-Za-z0-9-]+", entity["id"]), entity["id"]


@requires_docker
def test_every_entity_credits_its_publisher_and_its_licence(written):
    for entities in written.values():
        for entity in entities:
            assert "CC BY 4.0" in entity["dataProvider"]["value"], entity["id"]
            assert entity["source"]["value"].startswith("https://"), entity["id"]


@requires_docker
def test_an_event_keeps_the_citys_local_date_and_time_and_its_most_specific_category(written):
    events = {e["id"].rsplit("-", 1)[1]: e for e in written["podujatia"]}
    architecture = events["117711"]
    # The published day and time, as an instant with the city's summer offset (T-4329).
    assert architecture["startDate"]["value"] == "2026-10-05T16:00:00+02:00" and architecture["startTime"]["value"] == "16:00"
    assert architecture["endTime"]["value"] == "19:00"
    assert architecture["address"]["value"] == {"streetAddress": "Námestie SNP 1", "addressCountry": "SK",
                                                "addressLocality": "Banská Bystrica - Radnica, Cikkerova sieň"}
    assert architecture["location"]["value"] == {"type": "Point", "coordinates": [19.1459513, 48.7353768]}
    # WordPress's modified_gmt is UTC; `modified` would be two hours later in September.
    assert architecture["dateModified"]["value"] == "2026-09-24T10:41:20Z"
    # Filed under "other" and "museums" and "exhibitions": the first specific one wins.
    assert events["116585"]["eventCategory"]["value"] == "museumsGalleriesLibraries"
    # "&#8211;" in the rendered title is a dash to a reader.
    assert events["117447"]["name"]["languageMap"]["sk"] == "Potulky s cestovateľom – Maroko"
    # An event the city announces with no time has none, rather than midnight.
    assert "startTime" not in events["117687"]
    assert events["117687"]["startDate"]["value"].endswith("T00:00:00+02:00")
    for e in written["podujatia"]:
        assert open_data.sdm_errors("dataModel.TourismDestinations/Event", e) == [], e["id"]


@requires_docker
def test_a_post_filed_only_under_the_overviews_is_not_an_event():
    def edit(d):
        d[0]["event_categories"] = [459]
        d[1]["event_categories"] = [435, 460]
    assert len(run("podujatia", events_edited(edit))) == 3


@requires_docker
def test_a_title_holding_markup_is_published_as_its_text_and_a_title_of_markup_alone_is_no_event():
    # F-R2BNT1-002 (T-6215): WordPress allows inline tags in a title; the name is the text a reader sees.
    def edit(d):
        d[0]["title"]["rendered"] = "Koncert <em>Lúčnica</em>&nbsp;&#8211; <b>2026</b>"
        d[1]["title"]["rendered"] = "<br />"
    events = run("podujatia", events_edited(edit))
    assert len(events) == 4
    first = next(e for e in events if e["id"].endswith("-117711"))
    assert first["name"]["languageMap"]["sk"] == "Koncert Lúčnica\u00a0– 2026"


@requires_docker
def test_an_event_the_city_gives_no_start_day_is_left_out_never_dated():
    # T-4570: a start date is the source's own, never one the mapping makes up.
    def edit(d):
        del d[2]["meta"]["date_start"]
        d[3]["meta"]["date_start"] = ""
    events = run("podujatia", events_edited(edit))
    assert sorted(e["id"].rsplit("-", 1)[1] for e in events) == ["116585", "117687", "117711"]


@requires_docker
def test_an_unset_pin_or_a_bad_date_is_left_out_rather_than_written():
    def edit(d):
        d[0]["meta"]["coordinates"] = {"longitude": 0, "latitude": 0}
        d[0]["meta"]["time_start"] = "16h"
        d[1]["meta"]["date_start"] = "5. októbra"
    events = run("podujatia", events_edited(edit))
    assert len(events) == 4
    first = next(e for e in events if e["id"].endswith("-117711"))
    assert "location" not in first and "startTime" not in first and not schema_errors(first)


@requires_docker
def test_a_school_outside_the_city_is_dropped_and_its_numbers_read_as_numbers(written):
    schools = {e["schoolCode"]["value"]: e for e in written["skoly"]}
    assert "100003989" not in schools, "Považská Bystrica is another town"
    centre = schools["100009329"]
    assert centre["pupilCount"]["value"] == 1721
    assert centre["teachingStaff"]["value"] == 6.0 and centre["nonTeachingStaff"]["value"] == 3.5
    assert centre["annualBudget"]["value"] == 9702 and centre["budgetYear"]["value"] == 2024
    assert centre["location"]["value"] == {"type": "Point", "coordinates": [19.114874, 48.732009]}
    # "údaj nedostupný" and "Nesleduje sa" are the map saying it does not know.
    assert "pupilCount" not in schools["100009381"]
    assert "teachingLanguage" not in centre
    assert schools["100009344"]["teachingLanguage"]["value"] == "Slovenský"


@requires_docker
def test_the_budget_column_follows_its_year():
    text = recorded("skoly.csv").decode("utf-8-sig").replace('"Rozpočet v 2024"', '"Rozpočet v 2025"')
    schools = run("skoly", ("﻿" + text).encode())
    assert {e["budgetYear"]["value"] for e in schools} == {2025}


@requires_docker
def test_the_disadvantaged_pupils_count_never_reaches_the_space(written):
    text = json.dumps(written["skoly"], ensure_ascii=False)
    assert "znev" not in text and "disadvantaged" not in text


@requires_docker
def test_the_station_is_one_entity_and_each_stream_writes_its_own_reading(written):
    pm10, pm25 = written["ovzdusie-pm10"][0], written["ovzdusie-pm25"][0]
    assert pm10["id"] == pm25["id"] == f"urn:ngsi-ld:AirQualityObserved:{DOMAIN}:{SPACE}:eea-SK0263A"
    assert "pm25" not in pm10 and "pm10" not in pm25
    # The latest hour of the file is 05:00–06:00 UTC+1, which is 04:00–05:00 UTC.
    assert pm10["pm10"] == {"type": "Property", "value": 3.0746, "unitCode": "GQ", "observedAt": "2026-09-25T05:00:00Z"}
    assert pm25["pm25"]["value"] == 2.27 and pm25["pm25"]["observedAt"] == "2026-09-25T05:00:00Z"
    assert pm10["location"]["value"] == {"type": "Point", "coordinates": [19.115268, 48.733256]}


@requires_docker
def test_nitrogen_dioxide_and_ozone_join_the_same_station(written):
    """T-4153: each stream writes only its own pollutant into the one station entity, in µg/m³,
    stamped with the end of its hour in UTC."""
    no2, o3 = written["ovzdusie-no2"][0], written["ovzdusie-o3"][0]
    assert no2["id"] == o3["id"] == written["ovzdusie-pm10"][0]["id"]
    assert no2["no2"] == {"type": "Property", "value": 3.14639, "unitCode": "GQ", "observedAt": "2026-10-15T18:00:00Z"}
    assert o3["o3"] == {"type": "Property", "value": 51.03, "unitCode": "GQ", "observedAt": "2026-10-15T19:00:00Z"}
    for entity, own in ((no2, "no2"), (o3, "o3")):
        assert not {"pm10", "pm25", "no2", "o3"} - {own} & set(entity), entity


@requires_docker
def test_a_withdrawn_hour_is_skipped_for_the_one_before_it():
    station = run("ovzdusie-pm10", recorded("eea-sk0263a-pm10-withdrawn.parquet"))[0]
    assert station["pm10"]["observedAt"] == "2026-09-25T04:00:00Z"
    assert station["pm10"]["value"] == 3.4103


@requires_docker
def test_an_empty_answer_writes_nothing():
    assert run("podujatia", b"[]") == []
    header = recorded("skoly.csv").decode("utf-8-sig").splitlines()[0]
    assert run("skoly", ("﻿" + header + "\r\n").encode()) == []


def test_a_value_outside_the_model_is_refused_by_the_schema():
    event = {
        "id": f"urn:ngsi-ld:Event:{DOMAIN}:{SPACE}:event-1",
        "type": "Event",
        "startDate": {"type": "Property", "value": "2026-10-05T00:00:00+02:00"},
        "eventCategory": {"type": "Property", "value": "concert"},
    }
    assert schema_errors(event)
    event["eventCategory"]["value"] = "musicDanceTheatre"
    assert not schema_errors(event)
    event["startTime"] = {"type": "Property", "value": "4 pm"}
    assert schema_errors(event)


def test_every_feed_is_seeded_as_a_datasource_a_pipeline_and_a_mapping():
    index = yaml.safe_load((CITY / "index.yaml").read_text())
    model = manifest("datamodel-verejne.yaml")
    readable = {e["type"] for rule in manifest("policy-verejne-read.yaml")["spec"]["information"] for e in rule["entities"]}
    writable = {e["type"] for rule in manifest("policy-verejne-pipelines-write.yaml")["spec"]["information"] for e in rule["entities"]}
    for name, (_, kind, _) in FEEDS.items():
        pipeline = manifest(f"pipeline-{name}.yaml")
        datasource = manifest(f"datasource-{name}.yaml")
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"]
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:{DOMAIN}:{SPACE}:{SPACE}"
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 256
        assert datasource["spec"]["http"]["url"].startswith("https://")
        assert kind in model["spec"]["classes"] and kind in readable and kind in writable
        for seeded in (f"datasource-{name}.yaml", f"pipeline-{name}.yaml", f"pipeline-{name}-bento.yaml"):
            assert seeded in index, seeded


def test_the_events_query_never_asks_for_the_organisers_contact():
    """The feed carries the organiser's e-mail and telephone; `_fields` keeps them on the city's
    server, so a mapping that read them by mistake would still find nothing."""
    url = manifest("datasource-podujatia.yaml")["spec"]["http"]["url"]
    fields = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)["_fields"][0].split(",")
    assert "meta" not in fields, "the whole meta object would carry email and phone"
    assert not [f for f in fields if re.search(r"mail|phone|facebook", f)], fields


def test_every_poll_matches_its_source():
    periods = {name: manifest(f"pipeline-{name}.yaml")["spec"]["period"] for name in FEEDS}
    assert periods == {"podujatia": "24h", "skoly": "168h", "ovzdusie-pm10": "1h", "ovzdusie-pm25": "1h",
                       "ovzdusie-no2": "1h", "ovzdusie-o3": "1h"}
    # PL-26: 30 s or longer is scheduled; the two air streams do not start in the same minute.
    schedules = [manifest(f"pipeline-{name}.yaml")["spec"]["schedule"] for name in FEEDS]
    assert len(set(schedules)) == len(schedules)


def test_the_space_is_public_and_published_under_cc_by():
    endpoint = manifest("endpoint-verejne.yaml")
    space = manifest("space-verejne.yaml")
    assert endpoint["spec"]["audience"] == "public"
    # One dataset per type (T-4344): the space's Endpoint stays for the Apps, off the catalogue.
    assert "publish" not in endpoint["spec"]
    for part, kind in (("podujatia", "Event"), ("skoly", "Organization"), ("pocasie", "WeatherObserved")):
        spec = manifest(f"endpoint-{part}.yaml")["spec"]
        assert spec["audience"] == "public" and spec["contextSpaceRef"] == SPACE, part
        assert spec["publish"]["ckan"] == {
            "instanceRef": {"kind": "CkanInstance", "name": "banskabystrica"}, "license": "cc-by",
        }, part  # T-5060: no "datastore" member, CKAN holds no rows
        assert spec["catalog"]["license"] == "CC_BY_4_0", part
        policy = manifest(f"policy-{part}-read.yaml")["spec"]
        assert spec["policyRef"] == f"urn:ngsi-ld:Policy:banskabystrica.sk:{SPACE}:{part}-read", part
        assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == [kind], part
    assert space["spec"]["urnSegment"] == SPACE
    assert space["spec"]["dataModelRef"] == {"kind": "DataModel", "name": SPACE}
    for publisher in ("banskabystrica.sk", "Digitálna mapa škôl", "European Environment Agency", "CC BY 4.0"):
        assert publisher in space["metadata"]["description"]["en"], publisher


def test_the_pipelines_client_may_write_the_new_space():
    clients = yaml.safe_load((ROOT / "components/pipeline-runner/keycloak-clients.yaml").read_text())
    audiences = {
        mapper["config"]["included.custom.audience"]
        for mapper in clients["banskabystrica-pipelines"]["rawValues"]["protocolMappers"]
    }
    assert manifest("endpoint-verejne.yaml")["spec"]["slug"] in audiences
    roles = manifest("serviceaccount-pipelines.yaml")["spec"]["roles"]
    assert {"role": "space-writer", "scope": {"contextSpace": SPACE}} in roles


def test_the_model_classes_and_the_manifest_agree():
    linkml = yaml.safe_load((CITY / "banskabystrica-verejne.linkml.yaml").read_text())
    model = manifest("datamodel-verejne.yaml")
    assert linkml["name"] == model["metadata"]["name"]
    # PostalAddress is the value of `address`, never an entity type.
    assert set(model["spec"]["classes"]) == {k for k, c in linkml["classes"].items() if c.get("is_a") == "Entity"}


# T-3120: both EEA files into `ovzdusie`, the space `public-air` publishes, by one pipeline that
# expires what it stops renewing (PL-64).
AIR_SPACE = "ovzdusie"
AIR_SLUG = "mluyob4nz52lok3ssk7pgn5vwt"


def public_air(document: bytes) -> list[dict]:
    """Pipeline public-air's own steps, as the merged stream runs them, over one fetched file."""
    steps = manifest("pipeline-public-air.yaml")["spec"]["steps"]
    processors = [step["processor"] if "processor" in step else {"mapping": step["bloblang"]} for step in steps]
    # One file per call: the tests of this module run on several xdist workers at once, and a
    # shared name let one unlink the other's steps (main ci 40600747434).
    with tempfile.NamedTemporaryFile("w", dir=FIXTURES.parent, prefix=".public-air-steps-", suffix=".yaml",
                                     delete=False) as handle:
        handle.write(yaml.safe_dump({"pipeline": {"processors": processors}}))
    rendered = Path(handle.name)
    try:
        return open_data.run(rendered, document, AIR_SPACE, DOMAIN)
    finally:
        rendered.unlink()


@requires_docker
def test_public_air_reads_each_file_into_the_one_station_as_verejne_decodes_it():
    model = set(yaml.safe_load((CITY / "bb-air-quality.linkml.yaml").read_text())["classes"]["AirQualityObserved"]["slots"])
    for attribute in ("pm10", "pm25"):
        fixture = recorded(f"eea-sk0263a-{attribute}.parquet")
        [station] = public_air(fixture)
        [verejne] = run(f"ovzdusie-{attribute}", fixture)
        assert station["id"] == f"urn:ngsi-ld:AirQualityObserved:{DOMAIN}:{AIR_SPACE}:eea-SK0263A"
        # The file's Pollutant code picks the attribute; the other one is left to its own file.
        assert ({"pm10", "pm25"} - {attribute}).isdisjoint(station)
        assert station[attribute] == verejne[attribute]
        assert station["dateObserved"] == verejne["dateObserved"]
        # The catalogue smoke refuses a station without a name (T-4745); it is verejne's.
        assert station["name"] == verejne["name"]
        assert set(station) - {"id", "type", "location"} <= model, set(station) - model
        assert not air_schema_errors(station), air_schema_errors(station)


@requires_docker
def test_public_air_skips_a_withdrawn_hour_as_verejne_does():
    [station] = public_air(recorded("eea-sk0263a-pm10-withdrawn.parquet"))
    assert station["pm10"]["observedAt"] == "2026-09-25T04:00:00Z"


def air_schema_errors(entity: dict) -> list[str]:
    return open_data.schema_errors(CITY / "bb-air-quality.v1.schema.json", entity)


def test_public_air_is_the_spaces_one_writer_and_expires_what_it_stops_renewing():
    index = yaml.safe_load((CITY / "index.yaml").read_text())
    pipeline = manifest("pipeline-public-air.yaml")
    spec = pipeline["spec"]
    assert pipeline["apiVersion"] == "joinedcontext.com/v1alpha2"
    assert [s["dataSourceRef"]["name"] for s in spec["sources"]] == ["eea-sk0263a-pm10", "eea-sk0263a-pm25"]
    assert spec["outputs"] == [{
        "targetEndpoint": f"urn:ngsi-ld:Endpoint:{DOMAIN}:{AIR_SPACE}:public-air",
        "type": "AirQualityObserved", "mode": "upsert",
    }]
    assert spec["expiry"] == {"after": "7d", "types": ["AirQualityObserved"]}
    assert spec["period"] == "1h"
    schedules = [manifest(f"pipeline-{name}.yaml")["spec"]["schedule"] for name in (*FEEDS, "public-air")]
    assert len(set(schedules)) == len(schedules), "two streams start in the same minute"
    for seeded in ("policy-ovzdusie-pipelines-write.yaml", "pipeline-public-air.yaml"):
        assert seeded in index, seeded

    grant = manifest("policy-ovzdusie-pipelines-write.yaml")["spec"]
    assert grant["assignee"] == {"kind": "serviceAccount", "id": "pipelines"}
    assert grant["contextSpaceRef"]["name"] == AIR_SPACE
    assert set(grant["operations"]) == {"upsertBatch", "createBatch", "queryBatch", "deleteBatch"}
    roles = manifest("serviceaccount-pipelines.yaml")["spec"]["roles"]
    assert {"role": "space-writer", "scope": {"contextSpace": AIR_SPACE}} in roles
    clients = yaml.safe_load((ROOT / "components/pipeline-runner/keycloak-clients.yaml").read_text())
    audiences = {m["config"]["included.custom.audience"] for m in clients["banskabystrica-pipelines"]["rawValues"]["protocolMappers"]}
    assert AIR_SLUG in audiences
    # No person may delete there: the space's other grants name a role and a workload, never a user.
    assert manifest("policy-public-read.yaml")["spec"]["assignee"]["kind"] != "user"
    # The conformance suite writes its fixtures in its own space, not among the readings (F-BNT1-002).
    assert manifest("policy-conformance-write.yaml")["spec"]["contextSpaceRef"]["name"] == "conformance"


def test_the_dataset_says_how_often_it_changes_and_where_it_comes_from():
    endpoint = manifest("endpoint.yaml")
    assert endpoint["spec"]["slug"] == AIR_SLUG
    assert endpoint["spec"]["catalog"]["frequency"] == "HOURLY"
    for text in ("SK0263A", "European Environment Agency", "CC BY 4.0"):
        assert text in endpoint["metadata"]["description"]["en"], text
