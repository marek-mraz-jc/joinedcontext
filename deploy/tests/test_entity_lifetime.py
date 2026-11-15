"""T-4823, T-4843: a bus that stops reporting and a Digitraffic Alert that ended leave the space by
their own `expiresAt` (NGSI-LD 4.22), which the broker hides from every read once it passes and
then deletes. Two reaper pipelines did this before by reading the space back and batch-deleting
what they judged stale; they are gone, and so is the delete grant they needed beyond the one the
service-alerts withdrawal keeps (Alerts only). A vehicle's end is its pipeline's `spec.lifetime`
(PL-80), and so is a Fintraffic Alert's (`from: validTo, notAfter: PT1H`: its validTo, at most an
hour after the write, an hour after it without one, PL-82): the reconciler renders each into a
step after the mapping, so neither mapping writes `expiresAt` itself (PL-83 refuses both together).

Bento runs the committed mappings followed by the step the platform renders for the seeded
lifetime (tests/fixtures/lifetime); without a container runtime those cases skip, the seed cases
do not."""

import json
import re
from datetime import UTC, datetime, timedelta

import yaml
from open_data import requires_docker, run_all
from test_helsinki_open_data import HELSINKI
from test_hfp_vehicles import bento, frame

LIFETIME = HELSINKI.parents[3] / "tests/fixtures/lifetime"


def stamp(text: str) -> datetime:
    return datetime.fromisoformat(text)


def lifetime_step(pipeline: str) -> dict:
    return {"mapping": (LIFETIME / f"{pipeline}.blobl").read_text()}


def manifest(name: str) -> dict:
    return yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())


def test_the_vehicles_and_alerts_end_by_their_pipelines_lifetime():
    assert manifest("hfp")["spec"]["lifetime"] == [
        {"type": "Vehicle", "entity": {"from": "observedAt", "plus": "PT5M"}}]
    assert manifest("traffic-messages")["spec"]["lifetime"] == [
        {"type": "Alert", "entity": {"from": "validTo", "notAfter": "PT1H"}}]


def test_no_mapping_of_a_pipeline_with_a_lifetime_writes_expires_at_itself():
    """PL-83: admission refuses a mapping that assigns `expiresAt` beside a lifetime; the same
    patterns jc-core scans for, without a jcctl in this lane."""
    assigned = re.compile(r'(root|this)\s*\.\s*"?expiresAt"?\s*=(?!=)|"expiresAt"\s*:')
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    with_lifetime = [name for name in index if name.startswith("helsinki-pipeline-") and not name.endswith("-bento.yaml")
                     and (yaml.safe_load((HELSINKI / name).read_text()).get("spec") or {}).get("lifetime")]
    assert {"helsinki-pipeline-hfp.yaml", "helsinki-pipeline-traffic-messages.yaml"} <= set(with_lifetime)
    for name in with_lifetime:
        # A mapping lives in its own bento file or inline as `spec.compute.bloblang` (T-4857).
        mapping = HELSINKI / name.replace(".yaml", "-bento.yaml")
        text = mapping.read_text() if mapping.exists() else yaml.safe_load((HELSINKI / name).read_text())["spec"]["compute"]["bloblang"]
        code = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
        assert not assigned.search(code), name


def test_the_space_removes_an_alert_its_writer_gave_no_end_an_hour_after_its_last_write():
    space = yaml.safe_load((HELSINKI / "helsinki-space.yaml").read_text())
    rules = {r["entityType"]: r for r in space["spec"]["retention"]}
    assert rules["Alert"]["current"] == "PT1H"
    assert rules["Vehicle"]["current"] == "PT5M"


@requires_docker
def test_a_vehicle_expires_five_minutes_after_the_position_it_reports():
    (bus,) = bento([frame(7, "01", tst="2026-09-25T16:01:00.575Z")], threads=1, then=(lifetime_step("hsl-hfp-vehicles"),))
    assert bus["expiresAt"] == "2026-09-25T16:06:00Z"
    assert bus["location"]["observedAt"] == "2026-09-25T16:01:00.575Z"


@requires_docker
def test_a_report_whose_time_cannot_be_read_is_dropped_rather_than_living_forever():
    # The thirty-second sampling reads seconds 17..19; this one passes it and still is no time.
    frames = [frame(7, "01", tst="2026-09-25T16:01:00 nonsense")]
    assert bento(frames, threads=1, then=(lifetime_step("hsl-hfp-vehicles"),)) == []


def situation(n: int, ends: str | None) -> dict:
    timing = {"startTime": "2026-10-12T08:00:00Z"} | ({"endTime": ends} if ends else {})
    return {
        "geometry": {"type": "Point", "coordinates": [24.95, 60.17]},
        "properties": {
            "situationId": f"GUID7000{n}",
            "situationType": "ROAD_WORK",
            "releaseTime": "2026-10-12T08:00:00Z",
            "announcements": [{
                "title": "Tie 4, Helsinki",
                "locationDetails": {"roadAddressLocation": {"primaryPoint": {"municipality": "Helsinki"}}},
                "timeAndDuration": timing,
            }],
        },
    }


@requires_docker
def test_an_alert_expires_at_its_validto_or_an_hour_after_the_feed_last_listed_it():
    mapping = HELSINKI / "helsinki-pipeline-traffic-messages-bento.yaml"
    then = (lifetime_step("traffic-messages"),)
    before = datetime.now(UTC).replace(microsecond=0)
    soon = (before + timedelta(minutes=20)).strftime("%Y-%m-%dT%H:%M:%S+03:00")
    later = (before + timedelta(days=30)).strftime("%Y-%m-%dT%H:%M:%SZ")
    document = {"features": [situation(1, soon), situation(2, later), situation(3, None), situation(4, "2026-01-01T00:00:00Z")]}
    written = {e["id"][-1]: e for e in run_all(mapping, json.dumps(document).encode(), "helsinki", then=then)}
    after = datetime.now(UTC)
    # Ending within the hour: its own end, in UTC (the feed's offset is read, not dropped).
    assert stamp(written["1"]["expiresAt"]) == before + timedelta(minutes=20) - timedelta(hours=3)
    assert written["1"]["expiresAt"].endswith("Z")
    # Ending later, or never said: the hour the next run moves on while the feed lists it.
    for n in ("2", "3"):
        assert before + timedelta(hours=1) <= stamp(written[n]["expiresAt"]) <= after + timedelta(hours=1), written[n]["expiresAt"]
    # An end already past is written as expired, so no read shows it.
    assert written["4"]["expiresAt"] == "2026-01-01T00:00:00Z"


def test_no_reaper_pipeline_is_seeded():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert not [name for name in index if "reaper" in name or name.endswith("-live.yaml")], index
    assert not list(HELSINKI.glob("*reaper*"))
    for name in index:
        if name.startswith("helsinki-pipeline-") and name.endswith("-bento.yaml"):
            text = (HELSINKI / name).read_text()
            if "entityOperations/delete" in text:
                assert name == "helsinki-pipeline-service-alerts-bento.yaml", name


def test_the_pipelines_may_delete_only_alerts():
    """The reapers' deleteBatch over every type of the city space is gone: the pipelines account
    keeps a delete there only over Alerts, the one type a pipeline still withdraws."""
    deleting = []
    for path in sorted(HELSINKI.glob("helsinki-policy-*.yaml")):
        policy = yaml.safe_load(path.read_text())
        spec = policy["spec"]
        if spec["contextSpaceRef"]["name"] != "helsinki" or spec.get("assignee", {}).get("id") != "pipelines":
            continue
        if "deleteBatch" not in spec.get("operations", []):
            continue
        deleting.append(path.name)
        assert spec["operations"] == ["deleteBatch"], path.name
        assert [e["type"] for rule in spec["information"] for e in rule["entities"]] == ["Alert"], path.name
    assert deleting == ["helsinki-policy-pipelines-delete-alerts.yaml"], deleting
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert index["helsinki-policy-pipelines-delete-alerts.yaml"].endswith("/policies/pipelines-delete-alerts.yaml")


def test_one_history_default():
    """The platform's history default is the broker's ANTARES_RETENTION_HISTORY_DEFAULT; the
    `retention.history` of platform-settings.yaml had no reader and said something else."""
    settings = yaml.safe_load((HELSINKI / "helsinki-platform-settings.yaml").read_text())
    assert "retention" not in settings, json.dumps(settings)
