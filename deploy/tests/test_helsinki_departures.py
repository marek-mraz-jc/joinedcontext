"""The next departures of a curated set of HSL stops from GTFS-RT trip updates (T-3891, PL-03).

Bento runs the committed mapping with the runner's shared resources and the GTFS-RT schema it
mounts at /proto, over a cut of HSL's feed recorded on 2026-10-12 (13 trips, re-encoded from the
live document: `fixtures/helsinki/gtfs-rt/trip-updates.pb`) and over feeds the test encodes with
the same schema, each edited in the one thing it names. Without a container runtime the Bento
cases skip; the seed cases do not.
"""

import base64
import json
import re
import queue
import subprocess
import threading
import time
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-departures-bento.yaml"
STREAMS = ROOT / "components/pipeline-runner/streams"
FIXTURE = Path(__file__).resolve().parent / "fixtures/helsinki/gtfs-rt/trip-updates.pb"
URN = "urn:ngsi-ld:{}:hel.fi:helsinki:{}"
NOW = 1791774136
RAUTATIENTORI = "1020601"  # a metro platform of the list
OUTSIDE = "1173434"  # a bus stop of stops.txt that is not


# The runner's mounts: the shared resources at /streams, the GTFS-RT schema alone at /proto (T-4037).
MOUNTS = ["-v", f"{STREAMS}:/streams:ro", "-v", f"{STREAMS / 'gtfs-realtime.proto'}:/proto/gtfs-realtime.proto:ro"]


def docker(config: dict, document: bytes, codec: str = "lines") -> bytes:
    config = {**config, "output": {"stdout": {"codec": codec}}, "logger": {"level": "error"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", *MOUNTS, "-e", "JC_ORG_DOMAIN=hel.fi",
         "-e", "JC_SPACE=helsinki", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=120,
    )
    assert result.returncode == 0, result.stderr.decode()
    return result.stdout


def caches(ttl: str = "5m") -> list[dict]:
    resources = yaml.safe_load((STREAMS / "resources.yaml").read_text())["cache_resources"]
    for cache in resources:
        # The Portal renders the step's `departures` as the project's `helsinki__departures`
        # (T-5337); the mapping runs here as written, so the label is read back to its short name.
        cache["label"] = cache["label"].removeprefix("helsinki__")
        if cache["label"] == "departures":
            cache["memory"] = {"default_ttl": ttl, "compaction_interval": "1s"}
    return resources


def encode(feed: dict) -> bytes:
    """The protobuf HSL would send for `feed`, written with the schema the runner decodes by."""
    step = {"protobuf": {"operator": "from_json", "message": "transit_realtime.FeedMessage", "import_paths": ["/proto"]}}
    return docker({"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": [step]}},
                  json.dumps(feed).encode(), "all-bytes")


def run(document: bytes) -> dict[str, list[dict]]:
    """Each written stop's departures, by stop id."""
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"], "cache_resources": caches()}
    written = [json.loads(line) for line in docker(config, document).decode().splitlines() if line.strip()]
    for entity in written:
        assert entity["type"] == "GtfsStop" and set(entity) == {"id", "type", "departures"}, entity
        assert entity["departures"]["type"] == "JsonProperty"
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
    return {e["id"].rsplit(":", 1)[1]: e["departures"]["json"] for e in written}


def trip(route: str, *stops: tuple[str, int], cancelled: bool = False, skipped: tuple[str, ...] = ()) -> dict:
    return {"id": f"{route}-{stops[0][1]}", "tripUpdate": {
        "trip": {"routeId": route, "directionId": 1, "startTime": "05:34:00", "startDate": "20261012",
                 "scheduleRelationship": "CANCELED" if cancelled else "SCHEDULED"},
        "stopTimeUpdate": [{"stopId": stop, "departure": {"time": str(NOW + offset)},
                            "scheduleRelationship": "SKIPPED" if stop in skipped else "SCHEDULED"}
                           for stop, offset in stops],
    }}


def feed(*entities: dict) -> dict:
    return {"header": {"gtfsRealtimeVersion": "2.0", "incrementality": "FULL_DATASET", "timestamp": str(NOW)},
            "entity": list(entities)}


def listed() -> set[str]:
    """The stop ids of the mapping's list, read from its three arrays."""
    mapping = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][-2]["mapping"]
    return set(re.findall(r'"([0-9]{7})"', mapping.split("let wanted")[0]))


@requires_docker
def test_the_recorded_feed_gives_each_listed_stop_its_next_departures_in_order():
    stops = run(FIXTURE.read_bytes())
    assert set(stops) == listed() and len(stops) == 231
    served = {stop: rows for stop, rows in stops.items() if rows}
    # The cut's 13 trips touch 127 listed stops; at 74 of them a departure is still ahead of the
    # feed's own timestamp, and only those are shown.
    assert len(served) == 74, len(served)
    for rows in served.values():
        assert len(rows) <= 5
        assert [r["expected"] for r in rows] == sorted(r["expected"] for r in rows)
        for row in rows:
            assert row["route"] == URN.format("GtfsRoute", row["routeId"])
            assert row["status"] in {"scheduled", "skipped", "canceled"} and row["directionId"] in {0, 1}
            # HSL's v2 feed sends no delay, so no scheduled time is made up.
            assert "delay" not in row and "scheduled" not in row


@requires_docker
def test_five_soonest_from_now_and_nothing_outside_the_list():
    document = encode(feed(
        *(trip(f"10{n}", (RAUTATIENTORI, offset), (OUTSIDE, offset + 60)) for n, offset in enumerate((700, 100, 500, 300, 900, 200, 600))),
        trip("1099", (RAUTATIENTORI, -30)),
    ))
    stops = run(document)
    assert OUTSIDE not in stops
    rows = stops[RAUTATIENTORI]
    # Seven ahead, one already gone: the five soonest, soonest first.
    assert [r["routeId"] for r in rows] == ["101", "105", "103", "102", "106"]
    assert rows[0]["expected"] == "2026-10-12T03:03:56Z"


@requires_docker
def test_a_cancelled_trip_and_a_skipped_stop_stay_listed_and_say_so():
    with_delay = trip("1010", (RAUTATIENTORI, 400))
    with_delay["tripUpdate"]["stopTimeUpdate"][0]["departure"]["delay"] = 90
    stops = run(encode(feed(
        trip("1001", (RAUTATIENTORI, 100), cancelled=True),
        trip("1002", (RAUTATIENTORI, 200), skipped=(RAUTATIENTORI,)),
        trip("1003", (RAUTATIENTORI, 300)),
        with_delay,
        # Cancelled without its stops: it names no stop, so it is placed nowhere.
        {"id": "gone", "tripUpdate": {"trip": {"routeId": "1004", "scheduleRelationship": "CANCELED"}}},
    )))
    rows = stops[RAUTATIENTORI]
    assert [(r["routeId"], r["status"]) for r in rows] == [
        ("1001", "canceled"), ("1002", "skipped"), ("1003", "scheduled"), ("1010", "scheduled")]
    assert rows[3]["delay"] == 90 and rows[3]["scheduled"] == "2026-10-12T03:07:26Z"


@requires_docker
def test_an_empty_feed_keeps_the_departures_five_minutes_then_clears_them():
    """The cache's ttl, shortened to 2 s; each phase waits on what Bento wrote (T-2992)."""
    config = {"input": {"stdin": {"scanner": {"lines": {}}}},
              "pipeline": {"processors": [{"mapping": 'root = content().decode("base64")'},
                                          *yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]]},
              "cache_resources": caches("2s"), "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "error"}}
    full = base64.b64encode(encode(feed(trip("1003", (RAUTATIENTORI, 300))))).decode()
    empty = base64.b64encode(encode(feed())).decode()
    runner = subprocess.Popen(
        ["docker", "run", "--rm", "-i", *MOUNTS, "-e", "JC_ORG_DOMAIN=hel.fi",
         "-e", "JC_SPACE=helsinki", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    arrived: queue.Queue[str] = queue.Queue()
    threading.Thread(target=lambda: [arrived.put(line) for line in runner.stdout], daemon=True).start()

    def send(line: str) -> None:
        runner.stdin.write(line + "\n")
        runner.stdin.flush()

    def take(count: int) -> list[dict]:
        return [json.loads(arrived.get(timeout=60)) for _ in range(count)]

    try:
        send(full)
        first = {e["id"]: e["departures"]["json"] for e in take(231)}
        assert first[URN.format("GtfsStop", RAUTATIENTORI)][0]["routeId"] == "1003"
        # Within the ttl an empty feed writes nothing: the stops keep what they show.
        send(empty)
        time.sleep(1)
        assert arrived.empty()
        time.sleep(2)
        send(empty)
        cleared = take(231)
        assert all(e["departures"]["json"] == [] for e in cleared)
    finally:
        runner.kill()


@requires_docker
def test_a_body_too_large_or_not_a_feed_writes_nothing_and_clears_nothing():
    """Dropped as the runner drops a failed fetch: neither is read as an empty feed."""
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": yaml.safe_load(MAPPING.read_text())["pipeline"], "cache_resources": caches()}
    assert docker(config, b"\x0a" * (4 * 1024 * 1024 + 1)) == b""
    assert docker(config, b"<html>502 Bad Gateway</html>") == b""


def test_the_seed_reads_the_feed_every_30_s_into_gtfsstop_through_the_city_endpoint():
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-departures.yaml").read_text())["spec"]
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-trip-updates.yaml").read_text())
    assert pipeline["period"] == "30s" and pipeline["output"] == {"type": "GtfsStop", "mode": "upsert"}
    assert pipeline["source"]["dataSourceRef"]["name"] == source["metadata"]["name"] == "hsl-trip-updates"
    assert source["spec"]["http"]["url"] == "https://realtime.hsl.fi/realtime/trip-updates/v2/hsl"
    assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:{orgDomain}:helsinki:helsinki-all"
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for seeded in ("helsinki-datasource-trip-updates.yaml", "helsinki-pipeline-departures.yaml",
                   "helsinki-pipeline-departures-bento.yaml"):
        assert seeded in index, seeded
    # The step decodes by the schema the runner mounts beside its resources, and keeps it 5 min.
    step = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][1]["protobuf"]
    for processor in yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]:
        (name,) = processor
        assert name in {"mapping", "mutation", "protobuf", "branch", "unarchive"}, name
    assert step["import_paths"] == ["/proto"] and (STREAMS / "gtfs-realtime.proto").is_file()
    values = (ROOT / "components/pipeline-runner/values/runner/development-values.yaml.gotmpl").read_text()
    assert 'readFile "../../streams/gtfs-realtime.proto"' in values
    labels = yaml.safe_load((STREAMS / "resources.yaml").read_text())["cache_resources"]
    assert "helsinki__departures" in {c["label"] for c in labels}
    model = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    assert "departures" in model["classes"]["GtfsStop"]["slots"]
    assert model["slots"]["departures"]["annotations"]["ngsi_ld_kind"] == "JsonProperty"


def test_a_pass_never_copies_the_whole_feed_or_regroups_every_row():
    # T-4071: the runner was OOMKilled with this pass every 30 s beside 100 other streams. A
    # `mapping` answering `this` deep-copies the decoded feed (a mutation edits it in place), and
    # a `fold` that merges each row into the table so far copies the table on every row.
    processors = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    for processor in processors:
        mapping = processor.get("mapping", "")
        assert not re.search(r"\{\s*this\s*\}", mapping), mapping
    final = processors[-2]["mapping"]
    assert ".fold(" not in final.split("let rows")[1], "rows are taken per stop, not folded"
    assert ".filter(u -> $wanted.get(" in final, "only a kept stop's updates become rows"
