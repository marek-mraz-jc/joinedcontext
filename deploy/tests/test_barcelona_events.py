"""The Ajuntament's activity agendas become SDM Events, checked and counted (T-5265).

Bento runs the committed mapping over rows recorded from the bcn-agenda DataSource's DataStore
query on 2026-10-22 (`fixtures/barcelona/agenda/agenda-sql.json`): seven usable activities of
the three agendas, three rows the source really carries and a map cannot use (a point at 0,0, a
row with no position, a permanent collection with no start day) and two rows the test made (an
end before the start, the concert listed twice under a second id). Each bad row is dropped and
counted by its reason. Without a container runtime the Bento cases skip.
"""

import copy
import json
import re
import subprocess
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import parse_qs, urlsplit
from zoneinfo import ZoneInfo

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-open-data")

ROOT = Path(__file__).resolve().parent.parent
BARCELONA = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = BARCELONA / "barcelona-pipeline-agenda-bento.yaml"
FIXTURE = Path(__file__).resolve().parent / "fixtures/barcelona/agenda/agenda-sql.json"
URN = "urn:ngsi-ld:Event:barcelona.cat:barcelona:{}"
MADRID = ZoneInfo("Europe/Madrid")


def answer(modified: str | None = None, records: list | None = None) -> dict:
    """The recorded answer, its newest change moved to `modified` (now by default), so the
    staleness check judges the rows as the day's run would."""
    doc = json.loads(FIXTURE.read_text())
    if records is not None:
        doc["result"]["records"] = records
    stamp = modified or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S")
    for row in doc["result"]["records"][:1]:
        row["modified"] = stamp
    return doc


def run(doc: dict) -> tuple[list[dict], str, int]:
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"],
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "info", "format": "logfmt"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=barcelona.cat", "-e", "JC_SPACE=barcelona",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=json.dumps(doc).encode(), capture_output=True, timeout=120)
    lines = [line for line in result.stdout.decode().splitlines() if line.startswith("[")]
    events = [e for line in lines for e in json.loads(line)]
    return events, result.stderr.decode() + result.stdout.decode(), result.returncode


@requires_docker
def test_the_recorded_rows_map_to_valid_events_and_each_bad_row_is_dropped_by_reason():
    events, log, code = run(answer())
    assert code == 0, log
    by_id = {e["id"]: e for e in events}
    assert sorted(by_id) == sorted(URN.format(i) for i in (
        "99400013467", "99400642884", "99400255636", "2025121918", "99400300596", "2042164840"))
    assert ('dropped by reason {\\"duplicate\\":1,\\"ends before it starts\\":1,\\"no position\\":1,'
            '\\"outside Barcelona\\":1,\\"start day missing or before 2000\\":1}') in log, log
    for entity in events:
        assert not open_data.schema_errors(BARCELONA / "barcelona.v1.schema.json", entity), entity
        # The main category is one of the platform's fifteen codes (T-5363), which SDM's
        # suggested category list lacks; every other member passes the pinned SDM schema.
        errors = [e for e in open_data.sdm_errors("dataModel.TourismDestinations/Event", entity)
                  if not e.startswith("category/0")]
        assert not errors, (entity["id"], errors)
        lon, lat = entity["location"]["value"]["coordinates"]
        assert 2.04 <= lon <= 2.24 and 41.3 <= lat <= 41.48, entity["id"]
    concert = by_id[URN.format("99400642884")]
    assert concert["name"]["value"] == "Concert d’Any Nou del Gran Teatre del Liceu"
    assert concert["category"]["value"] == ["music", "spectacle"]
    assert concert["subCategory"]["value"] == ["music"]
    # A winter day starts at +01:00 in Barcelona, a summer one at +02:00.
    assert concert["startDate"]["value"] == "2027-01-04T00:00:00+01:00"
    assert concert["contactPoint"]["value"] == {"telephone": "934 859 913"}
    assert concert["source"]["value"] == "https://opendata-ajuntament.barcelona.cat/data/dataset/agenda-cultural"
    walk = by_id[URN.format("99400013467")]
    assert walk["category"]["value"] == ["sport"], "the sports agenda is sport whatever the name says"
    assert walk["address"]["value"] == {"streetAddress": "Passeig de Pujades 10", "postalCode": "08003",
                                        "addressLocality": "Barcelona", "district": "Ciutat Vella",
                                        "addressCountry": "ES"}
    workshop = by_id[URN.format("99400255636")]
    assert workshop["startDate"]["value"] == "2026-09-16T00:00:00+02:00"
    assert workshop["category"]["value"] == ["hobby-workshop"]
    assert by_id[URN.format("2025121918")]["category"]["value"] == ["exhibition-museum", "museum"]
    # A running activity with no last day carries no endDate rather than an empty one.
    assert "endDate" not in by_id[URN.format("2042164840")]


@requires_docker
@pytest.mark.parametrize("doc, message", [
    (answer(records=[]), "the Barcelona agendas list no current activity"),
    (answer(modified="2026-01-01T00:00:00"), "the Barcelona agendas are stale"),
    ({"success": False, "error": {"message": "relation does not exist"}}, "Open Data BCN refused the agenda query"),
])
def test_an_empty_stale_or_refused_answer_fails_the_run_instead_of_writing(doc, message):
    events, log, _ = run(copy.deepcopy(doc))
    # A thrown mapping error leaves the message flagged; the runner's output refuses a flagged
    # message, so the endpoint never sees an empty or an old list.
    assert message in log, log
    assert events == [] or all("id" not in e for e in events)


@requires_docker
def test_an_answer_that_reaches_the_query_limit_fails_the_run_instead_of_writing_part_of_the_agenda():
    """F-R2BNT1-004 (T-6215): the query ends in LIMIT N, so N rows may be a cut list."""
    source = yaml.safe_load((BARCELONA / "barcelona-datasource-agenda.yaml").read_text())
    limit = int(re.search(r"\bLIMIT (\d+)$", parse_qs(urlsplit(source["spec"]["http"]["url"]).query)["sql"][0]).group(1))
    row = answer()["result"]["records"][0]
    events, log, _ = run(answer(records=[{**row, "id": str(i)} for i in range(limit)]))
    assert f"the Barcelona agendas answered {limit} rows, the LIMIT {limit}" in log, log[-2000:]
    assert events == [] or all("id" not in e for e in events)
    events, log, code = run(answer(records=[{**row, "id": str(i)} for i in range(limit - 1)]))
    assert code == 0 and "LIMIT" not in log, log[-2000:]


def test_the_datasource_reads_the_three_agendas_in_one_query_from_open_data_bcn():
    source = yaml.safe_load((BARCELONA / "barcelona-datasource-agenda.yaml").read_text())
    url = urlsplit(source["spec"]["http"]["url"])
    assert (url.scheme, url.netloc, url.path) == (
        "https", "opendata-ajuntament.barcelona.cat", "/data/api/3/action/datastore_search_sql")
    sql = parse_qs(url.query)["sql"][0]
    for resource in ("877ccf66-9106-4ae2-be51-95a9f6469e4c", "3abb2414-1ee0-446e-9c25-380e938adb73",
                     "ab2d47b7-8493-4ed5-9f63-49ed116135f9"):
        assert f'"{resource}"' in sql
    assert sql.startswith("SELECT DISTINCT ON (id) ") and "end_date >= now() - interval '1 day'" in sql
    # Read only: one SELECT, no statement after it.
    assert ";" not in sql and not any(w in sql.upper() for w in ("INSERT ", "UPDATE ", "DELETE ", "DROP "))


def test_the_pipeline_runs_daily_through_the_one_door_and_the_endpoint_publishes_events_only():
    pipeline = yaml.safe_load((BARCELONA / "barcelona-pipeline-agenda.yaml").read_text())["spec"]
    assert pipeline["schedule"] == "17 6 * * *" and pipeline["output"] == {"type": "Event", "mode": "upsert"}
    assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:barcelona.cat:barcelona:barcelona"
    endpoint = yaml.safe_load((BARCELONA / "barcelona-endpoint-events.yaml").read_text())["spec"]
    assert endpoint["policyRef"] == "urn:ngsi-ld:Policy:barcelona.cat:barcelona:public-events"
    # Off the catalogue while prod holds none of it (T-5788, owner 2026-10-26): no publish block, harvest false.
    assert "publish" not in endpoint and endpoint["catalog"]["harvest"] is False
    assert {"ngsi-ld", "csv", "ogc-features", "odata"} <= set(endpoint["enabledRepresentations"])
    policy = yaml.safe_load((BARCELONA / "barcelona-policy-public-events.yaml").read_text())["spec"]
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["Event"]
    assert not {"upsertBatch", "createBatch", "deleteBatch", "createEntity"} & set(policy["operations"])


def madrid_day_end(day: date) -> datetime:
    """The local midnight after `day` in Barcelona, at the offset that midnight has."""
    return datetime.combine(day + timedelta(days=1), datetime.min.time(), MADRID).astimezone(timezone.utc)


def expected_expiry(ends: date | None, now: int) -> int:
    """The mapping's expiry rule, written again: the end of the last day or three days after the
    run, whichever comes first, and an hour after the run at least."""
    lease = now + 259200
    end = lease if ends is None else min(int(madrid_day_end(ends).timestamp()), lease)
    return max(end, now + 3600)


def expiry_map() -> str:
    """The `map expiry` definition, verbatim from the agenda mapping."""
    text = next(step["mapping"] for step in yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
                if "map expiry {" in step.get("mapping", ""))
    start = text.index("map expiry {")
    return text[start:text.index("\n}\n", start) + 2]


def at(stamp: str) -> int:
    return int(datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp())


@requires_docker
@pytest.mark.parametrize(("ends", "now", "expires"), [
    # 2026-03-29 turns to summer time at 01:00Z: the day before ends at its +01:00 midnight,
    # the day itself at the next day's +02:00 midnight.
    ("2026-03-28", "2026-03-28T10:00:00Z", "2026-03-28T23:00:00Z"),
    ("2026-03-29", "2026-03-29T10:00:00Z", "2026-03-29T22:00:00Z"),
    # 2026-10-25 turns back at 01:00Z: the day before ends at +02:00, the day itself at +01:00.
    ("2026-10-24", "2026-10-24T10:00:00Z", "2026-10-24T22:00:00Z"),
    ("2026-10-25", "2026-10-25T10:00:00Z", "2026-10-25T23:00:00Z"),
    # Within the last hour before that midnight the one-hour floor wins (run 40238119240's case).
    ("2026-10-25", "2026-10-25T22:05:09Z", "2026-10-25T23:05:09Z"),
    # An activity ending in a week, or with no end, leaves three days after the run.
    ("2026-11-01", "2026-10-25T10:00:00Z", "2026-10-28T10:00:00Z"),
    (None, "2026-03-29T00:30:00Z", "2026-04-01T00:30:00Z"),
])
def test_an_event_expires_at_the_madrid_midnight_after_its_last_day_on_both_summer_time_changes(ends, now, expires):
    assert expected_expiry(None if ends is None else date.fromisoformat(ends), at(now)) == at(expires)
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", BENTO, "blobl", expiry_map() + '\nroot = this.apply("expiry")'],
        input=json.dumps({"ends": ends, "now": at(now)}).encode() + b"\n", capture_output=True, timeout=60)
    assert result.returncode == 0, result.stderr.decode()
    assert int(result.stdout.decode().strip()) == at(expires), (result.stdout, result.stderr)


@requires_docker
def test_one_place_within_20_m_is_one_event_and_each_event_expires_by_itself():
    base = answer()["result"]["records"][1]
    # Bento reads its own clock inside the run: the expiry it writes lies between the rule at
    # the second before the run and at the second after it.
    before = int(datetime.now(timezone.utc).timestamp())
    today = datetime.fromtimestamp(before, MADRID).date()

    def row(rid, lat, name="Mercat de prova", ends="2027-01-04"):
        return dict(base, id=rid, name=name, lat=str(lat), lon="2.17335", ends=ends)
    # 41.38004 and 41.38013 lie 10 m apart on either side of a 0.0001-degree grid line; 41.38049
    # lies 50 m from the first; the last activity ends today.
    events, log, code = run(answer(records=[
        row("11", 41.38004), row("12", 41.38013), row("13", 41.38049),
        dict(row("14", 41.39, name="Taller d'avui", ends=today.isoformat()), starts=today.isoformat())]))
    after = int(datetime.now(timezone.utc).timestamp()) + 1
    assert code == 0, log
    by_id = {e["id"].rsplit(":", 1)[1]: e for e in events}
    assert len({"11", "12"} & set(by_id)) == 1 and {"13", "14"} <= set(by_id), sorted(by_id)
    assert '{\\"duplicate\\":1}' in log, log
    for rid, event in by_id.items():
        ends = date.fromisoformat(today.isoformat() if rid == "14" else "2027-01-04")
        expires = at(event["expiresAt"])
        assert expected_expiry(ends, before) <= expires <= expected_expiry(ends, after), (event, before, after)


@requires_docker
def test_a_day_of_a_summer_time_change_starts_at_the_offset_its_midnight_has():
    # 2026-10-25 and 2027-03-28 change at 01:00Z: their midnights are still +02:00 and +01:00,
    # their noons already +01:00 and +02:00.
    base = answer()["result"]["records"][1]
    events, log, code = run(answer(records=[
        dict(base, id="21", name="Tardor", starts="2026-10-25", ends="2027-03-28"),
        dict(base, id="22", name="Primavera", starts="2027-03-28", ends="2027-04-01")]))
    assert code == 0, log
    by_id = {e["id"].rsplit(":", 1)[1]: e for e in events}
    assert by_id["21"]["startDate"]["value"] == "2026-10-25T00:00:00+02:00"
    assert by_id["21"]["endDate"]["value"] == "2027-03-28T00:00:00+01:00"
    assert by_id["22"]["startDate"]["value"] == "2027-03-28T00:00:00+01:00"
