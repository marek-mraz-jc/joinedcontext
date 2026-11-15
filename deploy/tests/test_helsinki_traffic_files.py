"""Two HRI traffic count files become TrafficFlowObserved (T-5669, PL-03, DM-01).

Bento runs the committed traffic-volumes and vision-counts mappings over excerpts of the files read
on 2026-10-25 (`fixtures/helsinki/traffic-files`):
- hki_liikennemaarat.csv, the city's autumn traffic volume table: Lauttasaari bridge (A01) both
  directions, Paciuksenkatu on the D screen line (D01), Satamatunneli (C18, no direction) and
  Länsiväylä (B01) with one slot cut off, all of 2024, plus edited lines: a point moved to
  Kauniainen, a year ahead, a time that starts no slot, a count of 99999, a line with a letter for a
  count and A01's midnight slot written a second time with 1000 more vehicles.
- wapice_traffic_counts.json, the 2023 machine vision pilot: three lanes' first 50 hours (two whole
  days and two hours), with one negative count and one time not in ISO form, and a copy of a lane
  placed at 0,0.

Without a container runtime the Bento cases skip.
"""

import csv
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml

pytestmark = [pytest.mark.xdist_group("docker-helsinki-traffic-files"), open_data.requires_docker]

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
FIXTURES = Path(__file__).resolve().parent / "fixtures/helsinki/traffic-files"
URN = "urn:ngsi-ld:TrafficFlowObserved:hel.fi:helsinki:"


def run(mapping: str, document: bytes) -> tuple[dict, list[dict], int]:
    """The mapping over one fetched file: its entities by id suffix, its log lines and exit code."""
    pipeline = yaml.safe_load((HELSINKI / mapping).read_text())["pipeline"]
    # A failed run writes nothing: the runner refuses an errored message (its error is logged).
    pipeline["processors"].append({"catch": [{"mapping": "root = deleted()"}]})
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": pipeline,
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "info", "format": "json"}}
    folder = tempfile.mkdtemp()
    os.chmod(folder, 0o755)
    (Path(folder) / "c.json").write_text(json.dumps(config))
    os.chmod(Path(folder) / "c.json", 0o644)
    try:
        result = subprocess.run(
            ["docker", "run", "--rm", "-i", "--memory", "128m", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
             "-v", f"{folder}:/cfg:ro", open_data.BENTO, "-c", "/cfg/c.json"],
            input=document, capture_output=True, timeout=180, check=False,
        )
    finally:
        shutil.rmtree(folder)
    logs = [json.loads(line) for line in result.stderr.decode().splitlines() if line.startswith("{")]
    entities = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]
    assert all(e["type"] == "TrafficFlowObserved" for e in entities)
    assert len({e["id"] for e in entities}) == len(entities), "one id written twice"
    return {e["id"].removeprefix(URN): e for e in entities}, logs, result.returncode


def dropped(logs: list[dict], prefix: str) -> dict:
    (line,) = [log["msg"] for log in logs if log["level"] == "info" and log["msg"].startswith(prefix)]
    return json.loads(line.split("left out by reason ", 1)[1])


@pytest.fixture(scope="module")
def volumes():
    return run("helsinki-pipeline-traffic-volumes-bento.yaml", (FIXTURES / "hki_liikennemaarat.csv").read_bytes())


@pytest.fixture(scope="module")
def vision():
    return run("helsinki-pipeline-vision-counts-bento.yaml", (FIXTURES / "wapice_traffic_counts.json").read_bytes())


COUNTS = ("aika", "ha", "pa", "ka", "ra", "la", "mp", "rv", "autot")


def source_rows() -> list[dict]:
    with (FIXTURES / "hki_liikennemaarat.csv").open(encoding="utf-8-sig", newline="") as f:
        return list(csv.DictReader(f, delimiter=";"))


def test_a_day_per_point_direction_and_year_is_the_sum_of_its_slots(volumes):
    entities, _, code = volumes
    assert code == 0
    assert sorted(entities) == ["autumn-a01-1-2024", "autumn-a01-2-2024", "autumn-c18-both-2024", "autumn-d01-1-2024"]
    rows = source_rows()

    def total(point, direction):
        slots = {}
        for r in rows:
            if (r["piste"], r["suunta"], r["vuosi"]) == (point, direction, "2024") and all(r[c].isdigit() for c in COUNTS):
                slots.setdefault(r["aika"], []).append(int(r["autot"]))
        return slots

    a01 = total("A01", "1")
    assert len(a01) == 42 and len(a01["0"]) == 2
    # The repeated midnight slot: the later line stays.
    assert entities["autumn-a01-1-2024"]["intensity"]["value"] == sum(v[-1] for v in a01.values())
    assert entities["autumn-a01-2-2024"]["intensity"]["value"] == sum(v[0] for v in total("A01", "2").values())
    # Satamatunneli's two unnamed directions are one cross-section figure, without a direction.
    c18 = entities["autumn-c18-both-2024"]
    assert c18["intensity"]["value"] == sum(sum(v) for v in total("C18", "").values())
    assert "trafficDirection" not in c18


def test_directions_place_and_window(volumes):
    entities, _, _ = volumes
    a01 = entities["autumn-a01-1-2024"]
    assert a01["trafficDirection"]["value"] == "inbound"
    assert entities["autumn-a01-2-2024"]["trafficDirection"]["value"] == "outbound"
    # Direction 1 on the D screen line runs west.
    assert entities["autumn-d01-1-2024"]["trafficDirection"]["value"] == "west"
    assert a01["name"]["languageMap"] == {"fi": "LAUTTASAAREN SILTA"}
    assert a01["dateObservedFrom"]["value"] == "2024-08-31T21:00:00Z"
    assert a01["dateObservedTo"]["value"] == a01["dateObserved"]["value"] == a01["intensity"]["observedAt"] == "2024-10-31T22:00:00Z"
    for entity in entities.values():
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity["id"]
        assert not open_data.sdm_errors("dataModel.Transportation/TrafficFlowObserved", entity), entity["id"]


def test_gk25_positions_land_within_a_metre_of_pyproj(volumes):
    pyproj = pytest.importorskip("pyproj")
    to_wgs84 = pyproj.Transformer.from_crs(3879, 4326, always_xy=True)
    entities, _, _ = volumes
    points = {(r["piste"].lower(), r["vuosi"]): (int(r["x_gk25"]), int(r["y_gk25"])) for r in source_rows()}
    geod = pyproj.Geod(ellps="WGS84")
    for key, entity in entities.items():
        point, year = key.split("-")[1], key.split("-")[-1]
        lon, lat = entity["location"]["value"]["coordinates"]
        want = to_wgs84.transform(*points[(point, year)])
        assert geod.inv(lon, lat, *want)[2] < 1.0, key
    # The area gate is inside the space's area, so the cubic is used only where it was fitted.
    space = yaml.safe_load((HELSINKI / "helsinki-space.yaml").read_text())["spec"]
    (bbox,) = [r["bbox"] for r in space["quality"] if r.get("name") == "counter-in-helsinki"]
    for corner in ((25488000, 6660000), (25488000, 6688000), (25515000, 6660000), (25515000, 6688000)):
        lon, lat = to_wgs84.transform(*corner)
        assert bbox[0] <= lon <= bbox[2] and bbox[1] <= lat <= bbox[3], corner


def test_bad_lines_are_left_out_and_counted(volumes):
    _, logs, _ = volumes
    assert dropped(logs, "traffic-volumes:") == {
        "outsideHelsinki": 1, "badDate": 2, "implausibleCount": 1, "malformedRow": 1, "duplicateSlot": 1}
    # Länsiväylä lost a slot: its day would undercount, so it is not written, and it is said.
    (warning,) = [log["msg"] for log in logs if log["level"] == "warning"]
    assert "B01;1;2024 has 41 slot rows" in warning


def test_a_file_without_its_header_fails_and_writes_nothing():
    text = (FIXTURES / "hki_liikennemaarat.csv").read_text(encoding="utf-8-sig")
    entities, logs, _ = run("helsinki-pipeline-traffic-volumes-bento.yaml", text.split("\n", 1)[1].encode())
    assert entities == {}
    assert any("does not start with the header" in log["msg"] for log in logs if log["level"] == "error")


def test_vision_counts_a_vehicle_class_per_lane_and_whole_day(vision):
    entities, logs, code = vision
    assert code == 0
    assert len(entities) == 20
    document = json.loads((FIXTURES / "wapice_traffic_counts.json").read_text())
    lane6 = next(lane for lane in document if lane["id"] == 6)
    truck = entities["vision-6-lorry-20230404"]
    assert truck["intensity"]["value"] == sum(r["value"] for r in lane6["counts"]["truck"][:24])
    assert truck["vehicleType"]["value"] == "lorry"
    assert truck["trafficDirection"]["value"] == "south"
    assert truck["dateObservedFrom"]["value"] == "2023-04-03T21:00:00Z"
    assert truck["dateObservedTo"]["value"] == truck["intensity"]["observedAt"] == "2023-04-04T21:00:00Z"
    assert entities["vision-3-bus-20230404"]["trafficDirection"]["value"] == "north"
    assert entities["vision-4-bicycle-20230405"]["vehicleType"]["value"] == "bicycle"
    # The partial third day, the day with a negative count and the day with a bad time are not written.
    assert not [k for k in entities if k.endswith("20230406")]
    assert "vision-3-car-20230405" not in entities and "vision-3-bus-20230405" not in entities
    assert not [k for k in entities if k.startswith("vision-99-")]
    assert dropped(logs, "vision-counts:") == {
        "laneNoId": 0, "laneNoLocation": 0, "laneOutsideHelsinki": 1, "notAVehicle": 4,
        "hourBadTime": 1, "hourBadCount": 1, "hourRepeated": 0, "dayIncomplete": 13}
    for entity in entities.values():
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity["id"]
        assert not open_data.sdm_errors("dataModel.Transportation/TrafficFlowObserved", entity), entity["id"]


def test_vision_counts_refuse_a_file_that_is_not_a_lane_list():
    entities, logs, _ = run("helsinki-pipeline-vision-counts-bento.yaml", b'{"error": "gone"}')
    assert entities == {}
    assert any("not a list of lanes" in log["msg"] for log in logs if log["level"] == "error")


@pytest.mark.parametrize(("name", "source", "prefix", "own", "clock"), [
    ("traffic-volumes", "hel-traffic-volumes", "www.hel.fi", "autumn-a01-1-2024", "schedule"),
    ("vision-counts", "fvh-vision-counts", "dl.fvh.fi", "vision-6-lorry-20230404", None),
])
def test_each_file_is_one_pipeline_publishing_through_its_own_policy(name, source, prefix, own, clock):
    pipeline = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())["spec"]
    assert pipeline["source"]["dataSourceRef"]["name"] == source and pipeline["output"]["type"] == "TrafficFlowObserved"
    # The yearly file is read monthly; the closed pilot has no clock and runs once.
    assert "period" not in pipeline and ("schedule" in pipeline) == (clock == "schedule")
    datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())["spec"]["http"]
    assert datasource["url"].startswith(f"https://{prefix}/") and datasource["timeout"] == "60s"
    # The mapping fetches nothing itself and caps what it reads.
    bento = (HELSINKI / f"helsinki-pipeline-{name}-bento.yaml").read_text()
    assert "http:" not in bento and "MiB read" in bento
    policy = yaml.safe_load((HELSINKI / f"helsinki-policy-public-{name}.yaml").read_text())["spec"]
    (grant,) = policy["information"][0]["entities"]
    others = ["lam-4667-north", "cycle-4667-north", "autumn-a01-1-2024", "vision-6-lorry-20230404"]
    assert [o for o in others if re.match(grant["idPattern"], URN + o)] == [own]
    endpoint = yaml.safe_load((HELSINKI / f"helsinki-endpoint-{name}.yaml").read_text())
    assert endpoint["spec"]["policyRef"].endswith(f":public-{name}")
    assert endpoint["spec"]["catalog"]["pipelineRef"]["name"] == name
    assert all("trafficDirection" in text for text in endpoint["metadata"]["description"].values())
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for file in (f"helsinki-pipeline-{name}.yaml", f"helsinki-pipeline-{name}-bento.yaml", f"helsinki-datasource-{name}.yaml",
                 f"helsinki-endpoint-{name}.yaml", f"helsinki-policy-public-{name}.yaml"):
        assert file in index, file
