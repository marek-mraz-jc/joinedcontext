"""Bratislava's counter sites become TrafficFlowObserved and CrowdFlowObserved (T-5273, PL-03, DM-01).

Bento runs the committed counters and pedestrian-counters mappings over answers of the hub's
Feature Service tables (data.bratislava.sk) recorded on 2026-10-23 at 06:40 UTC
(`fixtures/bratislava/counters`): the newest row per site of cyklisti2_0, and the day 2026-10-21
(Bratislava time) of cyklisti2_0, vozidla_00 and chodci10_01 summed per site by the service.
Three records were edited per table the checks read: in the cyclists' day Hradza Berg got its
coordinates swapped, one hour of Dunajská towards the centre became 9,000 cyclists, and Starý Most
came a second time with the trailing newline the source writes into a label now and then; in the
pedestrians' day Viedenska sits at 0,0, River Park has one hour of -4 people and Most SNP repeats.
The recording also holds what the source gets wrong by itself: sites listed in a table whose sensor
they lack write 0 every hour (14 of the 19 vehicle sites).

The recorded newest rows are moved by whole days so that 2026-10-21 is yesterday in Bratislava, and
the stale rule (three days) judges the recording as it judged the live source. The calls to the
hub are the cut: each branch is answered from the recording in order.

Without a container runtime the Bento cases skip.
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
from datetime import date, datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

import open_data
import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-bratislava-counters")

ROOT = Path(__file__).resolve().parent.parent
BRATISLAVA = ROOT / "components/context-gateway/seed/bratislava"
FIXTURES = Path(__file__).resolve().parent / "fixtures/bratislava/counters"
SCHEMA = BRATISLAVA / "bratislava.v1.schema.json"
TZ = ZoneInfo("Europe/Bratislava")
RECORDED_DAY = date(2026, 10, 21)
STAMP = re.compile(r"(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})\+00:00")
BOX = (16.93, 47.99, 17.3, 48.28)


def newest(days_old: int = 1) -> str:
    """The recorded newest rows, moved so that the recorded day is `days_old` days before today."""
    shift = (datetime.now(TZ).date() - timedelta(days=days_old)) - RECORDED_DAY
    return STAMP.sub(lambda m: (datetime.fromisoformat(f"{m[1]}T{m[2]}") + shift).strftime("%Y-%m-%dT%H:%M:%S+00:00"),
                     (FIXTURES / "newest.json").read_text())


def day(table: str) -> dict:
    answer = json.loads((FIXTURES / f"{table}-day.json").read_text())
    return {"rows": [f["attributes"] for f in answer["features"]]}


def run(mapping: str, document: str, answers: list[dict], metadata: dict | None = None,
        ) -> tuple[list[dict], list[str], subprocess.CompletedProcess]:
    """The mapping with its n-th branch answered by answers[n], after the metadata a connection's
    own steps set (the member name of an archive read, PL-72); the entities it writes, the
    messages it logs and the process."""
    steps = [{"mapping": "\n".join(f"meta {k} = {json.dumps(v)}" for k, v in metadata.items())}] if metadata else []
    n = 0
    for step in yaml.safe_load((BRATISLAVA / mapping).read_text())["pipeline"]["processors"]:
        if "branch" in step:
            branch = dict(step["branch"])
            branch["processors"] = [{"mapping": f"root = {json.dumps(answers[n])}"}]
            step, n = {"branch": branch}, n + 1
        steps.append(step)
    assert n == len(answers)
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps},
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "warn", "format": "json"}}
    folder = tempfile.mkdtemp()
    os.chmod(folder, 0o755)
    (Path(folder) / "c.json").write_text(json.dumps(config))
    os.chmod(Path(folder) / "c.json", 0o644)
    try:
        result = subprocess.run(
            ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=bratislava.sk", "-e", "JC_SPACE=bratislava",
             "-v", f"{folder}:/cfg:ro", open_data.BENTO, "-c", "/cfg/c.json"],
            input=document, capture_output=True, text=True, timeout=180, check=False,
        )
    finally:
        shutil.rmtree(folder)
    logs = [json.loads(line) for line in result.stderr.splitlines() if line.startswith("{")]
    entities = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    assert len({e["id"] for e in entities}) == len(entities), "one id written twice"
    return entities, [f'{log["level"]}: {log["msg"]}' for log in logs], result


def by_id(entities: list[dict], kind: str) -> dict:
    prefix = f"urn:ngsi-ld:{kind}:bratislava.sk:bratislava:"
    assert all(e["id"].startswith(prefix) and e["type"] == kind for e in entities)
    return {e["id"].removeprefix(prefix): e for e in entities}


def needs_docker():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime, so Bento cannot run the mapping")


def midnight_utc(d: date) -> str:
    return datetime(d.year, d.month, d.day, tzinfo=TZ).astimezone(ZoneInfo("UTC")).strftime("%Y-%m-%dT%H:%M:%SZ")


@pytest.fixture(scope="module")
def traffic():
    needs_docker()
    entities, logs, result = run("bratislava-pipeline-counters-bento.yaml", newest(), [day("cyklisti2_0"), day("vozidla_00")])
    assert result.returncode == 0 and all(log.startswith("warning") for log in logs), result.stderr
    return by_id(entities, "TrafficFlowObserved"), logs


@pytest.fixture(scope="module")
def walkers():
    needs_docker()
    entities, logs, result = run("bratislava-pipeline-pedestrian-counters-bento.yaml", newest(), [day("chodci10_01")])
    assert result.returncode == 0 and all(log.startswith("warning") for log in logs), result.stderr
    return by_id(entities, "CrowdFlowObserved"), logs


def test_each_site_direction_and_mode_is_one_observation_of_the_closed_day(traffic):
    entities, _ = traffic
    yesterday = datetime.now(TZ).date() - timedelta(days=1)
    start, end = midnight_utc(yesterday), midnight_utc(yesterday + timedelta(days=1))
    # The recorded sums: Starý Most counted 454 cyclists towards the centre, 43 towards Petržalka.
    assert entities["cycle-1-forward"]["intensity"] == {"type": "Property", "value": 454, "observedAt": end}
    assert entities["cycle-1-backward"]["intensity"]["value"] == 43
    assert entities["cycle-1-forward"]["name"]["value"] == "#1 - Starý Most (Centrum)"
    assert entities["cycle-1-forward"]["vehicleType"]["value"] == "bicycle"
    assert "vehicleType" not in entities["vehicle-5-forward"]
    assert entities["cycle-viedenska-backward"]["laneDirection"]["value"] == "backward"
    for e in entities.values():
        assert e["dateObservedFrom"]["value"] == start and e["dateObservedTo"]["value"] == end == e["dateObserved"]["value"]
        lon, lat = e["location"]["value"]["coordinates"]
        assert BOX[0] <= lon <= BOX[2] and BOX[1] <= lat <= BOX[3]
        assert e["intensity"]["value"] >= 0


def test_one_way_sites_count_against_the_one_way_too(traffic):
    entities, _ = traffic
    # Vajanského 1 names no second direction ("NIE JE - v mieste je jednosmerná premávka") and still
    # counted 70 cyclists and 142 vehicles against the one-way street on the recorded day.
    assert entities["cycle-13-backward"]["intensity"]["value"] == 70
    assert entities["vehicle-13-backward"]["name"]["value"] == "#13 - Vajanského 1 (protismer)"


def test_the_edited_records_are_left_out_with_a_counted_reason(traffic):
    entities, logs = traffic
    assert not any(k.startswith("cycle-hradza-berg") for k in entities), "swapped coordinates written"
    assert "cycle-10-forward" not in entities and "cycle-10-backward" in entities, "the 9,000 an hour hour written"
    # The repeated Starý Most counts 48 hours for one day: the first stays, the repeat goes.
    assert entities["cycle-1-forward"]["intensity"]["value"] == 454
    (line,) = [log for log in logs if "counters left out" in log]
    assert json.loads(line.split("left out: ", 1)[1]) == {
        "cycle:swappedCoordinates": 1, "cycle:outOfRange": 1, "cycle:duplicate": 1, "cycle:zeroDay": 2, "vehicle:zeroDay": 15}
    # Every other recorded site and direction is written: of 23 cyclist sites Vajanského 2 and River
    # Park counted 0 all day; of 19 vehicle sites 15 did, 14 of them for the whole month before.
    assert len([k for k in entities if k.startswith("cycle-")]) == 23 * 2 - 2 - 1 - 2 * 2
    assert sorted(k for k in entities if k.startswith("vehicle-")) == [
        f"vehicle-{n}-{d}" for n in (10, 13, 17, 5) for d in ("backward", "forward")]


def test_every_observation_passes_the_smart_data_model_and_the_space_model(traffic):
    entities, _ = traffic
    for e in entities.values():
        assert open_data.sdm_errors("dataModel.Transportation/TrafficFlowObserved", e) == [], e["id"]
        assert open_data.schema_errors(SCHEMA, e) == [], e["id"]


def test_pedestrians_are_one_crowd_flow_per_site_both_directions_apart(walkers):
    entities, logs = walkers
    one = entities["pedestrian-1"]
    assert (one["peopleCountTowards"]["value"], one["peopleCountAway"]["value"]) == (990, 455)
    assert one["peopleCount"]["value"] == 990 + 455
    assert one["name"]["value"] == "#1 - Starý Most (Centrum / Petržalka)"
    assert entities["pedestrian-13"]["peopleCountAway"]["value"] == 1070
    assert "pedestrian-viedenska" not in entities and "pedestrian-3" not in entities
    (line,) = [log for log in logs if "left out" in log]
    assert json.loads(line.split("left out: ", 1)[1]) == {"outsideBratislava": 1, "outOfRange": 1, "duplicate": 1, "zeroDay": 3}
    # Devínska Nová Ves, Hrádza Berg and Vajanského 2 counted no pedestrian all day.
    assert len(entities) == 23 - 5
    for e in entities.values():
        assert open_data.sdm_errors("dataModel.Transportation/CrowdFlowObserved", e) == [], e["id"]
        assert open_data.schema_errors(SCHEMA, e) == [], e["id"]


def test_a_source_silent_for_three_days_fails_the_run_and_writes_nothing():
    needs_docker()
    entities, logs, result = run("bratislava-pipeline-counters-bento.yaml", newest(days_old=4), [day("cyklisti2_0"), day("vozidla_00")])
    assert entities == [] and any("more than three days old" in log for log in logs), result.stderr


def test_one_table_refusing_the_query_leaves_the_other_written():
    needs_docker()
    entities, logs, result = run("bratislava-pipeline-counters-bento.yaml", newest(), [{"failed": True}, day("vozidla_00")])
    assert entities and all(e["id"].split(":")[-1].startswith("vehicle-") for e in entities), result.stderr
    entities, logs, result = run("bratislava-pipeline-counters-bento.yaml", newest(), [{"failed": True}, {"failed": True}])
    assert entities == [] and any("no counter site wrote a count" in log for log in logs), result.stderr


def test_the_day_query_names_the_closed_day_and_reads_one_page():
    mapping = (BRATISLAVA / "bratislava-pipeline-counters-bento.yaml").read_text()
    for table in ("cyklisti2_0", "vozidla_00"):
        assert f"/{table}/FeatureServer/0/query?where=DATUM_A_CAS%20%3E%3D%20%27${{! @from }}%27%20AND%20DATUM_A_CAS%20%3C%20%27${{! @to }}%27" in mapping
    assert mapping.count("exceededTransferLimit") == 2 and mapping.count("this.error.or(null)") == 2
