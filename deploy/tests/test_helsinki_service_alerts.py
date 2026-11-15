"""HSL's GTFS-RT service alerts become Alert entities linked to their lines and stops (T-3892).

Bento runs the committed mapping over HSL's whole alerts feed recorded on 2026-10-12 (18 alerts,
`fixtures/helsinki/gtfs-rt/service-alerts.pb`) and over feeds the test encodes. The two calls to
the gateway are the cut: the lookup of the lines and stops the space holds is answered by the
test, and the withdrawal's mapping runs on its own over what the space holds and what the feed
lists. Without a container runtime the Bento cases skip; the seed cases do not.
"""

import json
import re
from datetime import UTC, datetime

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_helsinki_departures import HELSINKI, NOW, URN, docker, encode, feed

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

MAPPING = HELSINKI / "helsinki-pipeline-service-alerts-bento.yaml"
FIXTURE = HELSINKI.parents[3] / "tests/fixtures/helsinki/gtfs-rt/service-alerts.pb"
HSL = "https://realtime.hsl.fi/realtime/service-alerts/v2/hsl"
START, END = 1791968400, 1792429200


def processors() -> list[dict]:
    return yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]


def calls_gateway(processor: dict) -> bool:
    return "branch" in processor and "JC_GATEWAY" in json.dumps(processor)


def run(document: bytes, known: dict) -> dict[str, dict]:
    """The Alerts written, by id, with the lookup answered `known` and no withdrawal."""
    steps = []
    for processor in processors():
        if calls_gateway(processor) and "entityOperations/query" in json.dumps(processor):
            steps.append({"mutation": f"root.known = {json.dumps(known)}"})
        elif not calls_gateway(processor):
            steps.append(processor)
    out = docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps}}, document)
    written = [json.loads(line) for line in out.decode().splitlines() if line.strip()]
    for entity in written:
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
        assert entity["type"] == "Alert" and entity["source"]["value"] == HSL
    return {e["id"].rsplit(":", 1)[1]: e for e in written}


def held(*urns: str) -> dict:
    return {"held": list(urns)}


def alert(n: int, *informed: dict, end: bool = True, severity: str = "WARNING") -> dict:
    period = {"start": str(START)} | ({"end": str(END)} if end else {})
    text = lambda words: {"translation": [{"text": f"{words} {lang}", "language": lang} for lang in ("fi", "sv", "en")]}
    return {"id": str(n), "alert": {
        "activePeriod": [period], "informedEntity": list(informed), "cause": "CONSTRUCTION",
        "effect": "DETOUR", "headerText": text("Header"), "descriptionText": text("Body"),
        "severityLevel": severity,
    }}


@requires_docker
def test_every_recorded_alert_is_written_in_three_languages():
    alerts = run(FIXTURE.read_bytes(), held())
    assert len(alerts) == 18
    tapanila = alerts["hsl-315885"]
    assert set(tapanila["name"]["languageMap"]) == {"fi", "sv", "en"}
    assert tapanila["effect"]["value"] == "DETOUR" and tapanila["cause"]["value"] == "CONSTRUCTION"
    # GTFS-RT's effect has no Smart Data Models code, so the Alert carries none (T-3950).
    assert tapanila["alertSource"]["value"] == "HSL" and "subCategory" not in tapanila
    assert tapanila["severity"]["value"] == "informational"
    assert tapanila["validFrom"]["value"] == "2026-09-05T06:23:00Z"
    # dataModel.Alert requires dateIssued (T-4002): the first active period's start.
    assert tapanila["dateIssued"]["value"] == "2026-09-05T06:23:00Z"
    assert all("dateIssued" in a for a in alerts.values())
    # The space held none of what the alerts name here, so every name stays text.
    assert tapanila["unknownReference"]["value"] == ["stop:1392501", "stop:1392551"]
    assert not any("refRoute" in a or "refStop" in a for a in alerts.values())


@requires_docker
def test_a_route_and_a_stop_the_space_holds_are_linked_an_unknown_one_stays_text():
    route, stop = URN.format("GtfsRoute", "4300"), URN.format("GtfsStop", "1392501")
    alerts = run(encode(feed(
        alert(1, {"routeId": "4300"}, {"routeId": "100HE"}),
        alert(2, {"stopId": "1392501"}, end=False, severity="SEVERE"),
        alert(3, {"agencyId": "HSL"}, severity="UNKNOWN_SEVERITY"),
    )), held(route, stop))
    on_route, on_stop, on_agency = alerts["hsl-1"], alerts["hsl-2"], alerts["hsl-3"]
    assert on_route["refRoute"] == {"type": "Relationship", "object": [route]}
    assert on_route["unknownReference"]["value"] == ["route:100HE"]
    assert on_route["validTo"]["value"] == "2026-10-19T17:00:00Z" and on_route["severity"]["value"] == "medium"
    assert on_stop["refStop"] == {"type": "Relationship", "object": [stop]} and "refRoute" not in on_stop
    # No end to its active period: open-ended, so no validTo.
    assert "validTo" not in on_stop and on_stop["validFrom"]["value"] == "2026-10-14T09:00:00Z"
    assert on_stop["severity"]["value"] == "high"
    assert not {"refRoute", "refStop", "unknownReference", "severity"} & set(on_agency)
    assert on_agency["name"]["languageMap"]["sv"] == "Header sv"


@requires_docker
def test_an_alert_without_a_dated_period_is_issued_when_the_feed_was_produced():
    undated = alert(1, {"agencyId": "HSL"})
    undated["alert"]["activePeriod"] = []
    later = alert(2, {"agencyId": "HSL"})
    later["alert"]["activePeriod"].append({"start": str(START - 3600)})
    alerts = run(encode(feed(undated, later)), held())
    produced = datetime.fromtimestamp(NOW, UTC)
    assert alerts["hsl-1"]["dateIssued"]["value"] == produced.strftime("%Y-%m-%dT%H:%M:%SZ")
    assert "validFrom" not in alerts["hsl-1"]
    # Two periods, both after the feed was produced: an issue time never lies ahead (T-5259).
    assert alerts["hsl-2"]["dateIssued"]["value"] == produced.strftime("%Y-%m-%dT%H:%M:%SZ")
    assert alerts["hsl-2"]["validFrom"]["value"] == "2026-10-14T08:00:00Z"


@requires_docker
def test_an_alert_whose_period_began_before_the_feed_is_issued_at_its_start():
    begun = alert(1, {"agencyId": "HSL"})
    begun["alert"]["activePeriod"] = [{"start": str(NOW - 7200)}, {"start": str(NOW - 3600)}]
    alerts = run(encode(feed(begun)), held())
    started = datetime.fromtimestamp(NOW - 7200, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    assert alerts["hsl-1"]["dateIssued"]["value"] == started == alerts["hsl-1"]["validFrom"]["value"]


@requires_docker
def test_the_lookup_asks_for_exactly_the_named_lines_and_stops():
    steps = processors()
    lookup = next(p for p in steps if calls_gateway(p) and "entityOperations/query" in json.dumps(p))["branch"]
    named = steps[:steps.index(next(p for p in steps if "branch" in p))]
    ask = {"mapping": lookup["request_map"]}
    request = lambda document: docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}},
                                       "pipeline": {"processors": [*named, ask]}}, document)
    asked = json.loads(request(encode(feed(alert(1, {"routeId": "4300"}, {"stopId": "1392501"}, {"routeId": "4300"})))))
    assert asked == {"type": "Query", "entities": [
        {"id": URN.format("GtfsRoute", "4300"), "type": "GtfsRoute"},
        {"id": URN.format("GtfsStop", "1392501"), "type": "GtfsStop"}]}
    # Nothing named, nothing asked: the branch is skipped.
    assert request(encode(feed(alert(1, {"agencyId": "HSL"})))) == b""


@requires_docker
def test_a_failed_lookup_writes_nothing_and_a_bad_body_neither():
    assert run(encode(feed(alert(1, {"routeId": "4300"}))), {"failed": True}) == {}
    assert run(b"<html>502 Bad Gateway</html>", held()) == {}
    assert run(b"\x0a" * (1024 * 1024 + 1), held()) == {}


@requires_docker
def test_an_alert_hsl_withdrew_is_deleted_and_no_other_alert_is():
    branch = next(p for p in processors() if calls_gateway(p) and "entityOperations/delete" in json.dumps(p))["branch"]
    decide = next(p for p in branch["processors"][0]["try"] if "mapping" in p)

    def withdrawn(current: list[str], space: list[dict]) -> list[list[str]]:
        out = docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": [decide]}},
                     json.dumps({"current": current, "held": space}).encode())
        return [json.loads(line) for line in out.decode().splitlines() if line.strip()]

    a = lambda n, source=HSL: {"id": URN.format("Alert", f"hsl-{n}"), "source": source}
    space = [a(1), a(2), a(3, "https://tie.digitraffic.fi/api/traffic-message/v1/messages"), {"id": URN.format("Alert", "x")}]
    # Alert 2 was in the last fetch and is not in this one.
    assert withdrawn([URN.format("Alert", "hsl-1")], space) == [[URN.format("Alert", "hsl-2")]]
    # HSL lists no alert at all: every HSL Alert goes, the others stay.
    assert withdrawn([], space) == [[URN.format("Alert", "hsl-1"), URN.format("Alert", "hsl-2")]]
    assert withdrawn([URN.format("Alert", "hsl-1"), URN.format("Alert", "hsl-2")], space) == [[]]
    # The delete is sent only for a non-empty list.
    switch = next(p for p in branch["processors"][0]["try"] if "switch" in p)["switch"]
    assert switch[0]["check"] == "this.length() > 0" and "entityOperations/delete" in json.dumps(switch[0])


def test_the_seed_reads_the_feed_every_minute_into_alert_through_the_city_endpoint():
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-service-alerts.yaml").read_text())["spec"]
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-service-alerts.yaml").read_text())
    assert pipeline["period"] == "60s" and pipeline["output"] == {"type": "Alert", "mode": "upsert"}
    assert pipeline["source"]["dataSourceRef"]["name"] == source["metadata"]["name"] == "hsl-service-alerts"
    assert source["spec"]["http"]["url"] == HSL
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for seeded in ("helsinki-datasource-service-alerts.yaml", "helsinki-pipeline-service-alerts.yaml",
                   "helsinki-pipeline-service-alerts-bento.yaml"):
        assert seeded in index, seeded
    public = yaml.safe_load((HELSINKI / "helsinki-policy-public-alerts.yaml").read_text())
    assert "Alert" in {e["type"] for rule in public["spec"]["information"] for e in rule["entities"]}
    model = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    assert {"cause", "severity", "refRoute", "refStop", "unknownReference"} <= set(model["classes"]["Alert"]["slots"])
    assert model["slots"]["refRoute"]["inverse"] == "routeAlerts" and model["slots"]["refStop"]["inverse"] == "stopAlerts"


@requires_docker
def test_a_value_gtfs_rt_adds_later_leaves_the_attribute_out_and_keeps_the_alert():
    late = alert(1, {"routeId": "1001"})
    late["alert"] |= {"effect": 99, "cause": 99}
    written = run(encode(feed(late)), held())["hsl-1"]
    assert "effect" not in written and "cause" not in written and written["severity"]["value"] == "medium"


def test_the_pipeline_writes_exactly_the_model_enums():
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())["definitions"]
    mapping = (HELSINKI / "helsinki-pipeline-service-alerts-bento.yaml").read_text()
    for let, enum in (("effects", "AlertEffect"), ("causes", "AlertCause")):
        listed = json.loads(re.search(rf"let {let} = (\[.*\])", mapping).group(1))
        assert listed == schema[enum]["enum"]
    assert set(json.loads(re.search(r"let severity = (\{.*\})", mapping).group(1)).values()) == set(schema["AlertSeverity"]["enum"])
