"""hsl-hfp-vehicles writes every report of every HSL vehicle, one write per batch (T-5046, T-5247).

Run by Bento over HFP frames with the committed mapping and the runner's resources, the input
batched as the reconciler renders an MQTT source. It once capped the fleet per mode with slots
in a cache (T-2961, T-3893) and wrote one report in thirty seconds (T-4624); the owner asked for
every vehicle at its current position (option A) and every per-second change for a subscriber, and then
(T-5247) the complete history of the last five minutes, so a batch of up to a second of frames is one write
holding each vehicle's every report, an exact repeat once.
"""

import json
import queue
import subprocess
import threading
import time
from pathlib import Path

import yaml

import open_data
from open_data import BENTO, requires_docker

ROOT = Path(__file__).resolve().parent.parent
MAPPING = ROOT / "components/context-gateway/seed/helsinki/helsinki-pipeline-hfp-bento.yaml"
RESOURCES = ROOT / "components/pipeline-runner/streams/resources.yaml"


def topic(mode: str, oper: int, vehicle: int, route: str, next_stop: str = "1040601") -> str:
    """The HFP topic a frame arrives on; the mapping reads the mode and the next stop from it."""
    return f"/hfp/v2/journey/ongoing/vp/{mode}/{oper:04d}/{vehicle:05d}/{route}/1/Kamppi/16:00/{next_stop}/5/60;24/19/75/45"


def frame(vehicle: int, phase: str, mode: str = "bus", oper: int = 22, route: str = "2550", **fields) -> str:
    vp = {"oper": oper, "veh": vehicle, "long": 24.94, "lat": 60.17, "spd": 8.5, "hdg": 90,
          "route": route, "tst": f"2026-09-25T16:{phase}:00.000Z"} | fields
    return json.dumps({"topic": topic(mode, oper, vehicle, route), "VP": vp})


def pipeline(then: tuple[dict, ...] = (), **extra) -> dict:
    """The committed mapping behind a step that sets the topic as the runner's MQTT input does,
    and before the steps `then` (the reconciler's lifetime step, T-4843)."""
    committed = yaml.safe_load(MAPPING.read_text())["pipeline"]
    # A line's `raw` is the frame's bytes as they came, for a frame that is not JSON.
    as_mqtt = {"mapping": 'meta mqtt_topic = this.topic\nroot = if this.exists("raw") { this.raw } else { this.without("topic") }'}
    return {**committed, "processors": [as_mqtt, *committed["processors"], *then], **extra}


@requires_docker
def test_a_report_without_a_gps_fix_is_dropped_and_a_missing_speed_is_left_out():
    """T-3129: HFP sends position reports with no fix (`loc: "ODO"`, lat/long/spd/hdg null).

    Built into a Vehicle they failed on `long.number()` and every one was kept as a rejected
    record: 1,000 piled up on dev and buried the real ones. A report without coordinates says
    nothing about where the bus is and is dropped; a report with
    coordinates and no speed or heading is written without them.
    """
    def report(vehicle: int, **fields) -> str:
        vp = {"oper": 22, "veh": vehicle, "long": 24.94, "lat": 60.17, "spd": 8.5, "hdg": 90,
              "route": "2510", "tst": "2026-10-06T17:26:30.575Z", "loc": "GPS"}
        vp.update(fields)
        return json.dumps({"topic": topic("bus", 22, vehicle, "2510"), "VP": vp})

    frames = [
        report(1, lat=None, long=None, spd=None, hdg=None, loc="ODO"),
        report(2, spd=None, hdg=None),
        report(3),
    ]
    entities, counters = run(frames)
    assert errors(counters) == 0, counters
    written = {entity["fleetVehicleId"]["value"]: entity for entity in entities}
    assert sorted(written) == ["22-2", "22-3"]
    without_speed, whole = written["22-2"], written["22-3"]
    assert without_speed["location"]["value"]["coordinates"] == [24.94, 60.17]
    assert "speed" not in without_speed and "heading" not in without_speed
    assert whole["speed"]["value"] == 8.5 and whole["heading"]["value"] == 90


def short_caches() -> list[dict]:
    """The runner's caches under the names the mapping writes: the Portal renders a step's `gtfs`
    as the project's `helsinki__gtfs` (T-5337), and the mapping runs here as written."""
    caches = yaml.safe_load(RESOURCES.read_text())["cache_resources"]
    assert all(cache["label"].startswith("helsinki__") for cache in caches), caches
    for cache in caches:
        cache["label"] = cache["label"].removeprefix("helsinki__")
    return caches


def run(frames: list[str], known: tuple[str, ...] = (), then: tuple[dict, ...] = (),
        limit: int | None = None, **extra) -> tuple[list[dict], list[dict]]:
    """The mapping over `frames` in batches of up to 1,000 or a second, as the reconciler renders an
    MQTT input (T-5046), with the `gtfs` cache holding the URNs `known` as the daily GTFS read
    leaves them; `limit` sets the reports admitted a second. Returns
    what was written and Bento's counters."""
    runner = yaml.safe_load(RESOURCES.read_text())
    runner["cache_resources"] = short_caches()
    for cache in runner["cache_resources"]:
        if cache["label"] == "gtfs":
            cache["memory"]["init_values"] = {urn: "1" for urn in known}
    if limit:
        for rate in runner["rate_limit_resources"]:
            if rate["label"] == "hfp_vehicles":
                rate["local"]["count"] = limit
    config = {
        "input": {"broker": {"inputs": [{"stdin": {"scanner": {"lines": {}}}}],
                             "batching": {"count": 1000, "period": "1s"}}},
        "pipeline": pipeline(then, **extra),
        "cache_resources": runner["cache_resources"],
        "rate_limit_resources": runner["rate_limit_resources"],
        "output": {"stdout": {"codec": "lines"}},
        "metrics": {"logger": {"push_interval": "", "flush_metrics": True}},
        "logger": {"level": "info", "format": "json"},
    }
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        check=False, input="".join(line + "\n" for line in frames), capture_output=True, text=True, timeout=120,
    )
    assert result.returncode == 0, result.stderr
    counters = [json.loads(line) for line in result.stderr.splitlines() if '"Counter metric"' in line]
    return [json.loads(line) for line in result.stdout.splitlines() if line.strip()], counters


def bento(frames: list[str], known: tuple[str, ...] = (), then: tuple[dict, ...] = (), **extra) -> list[dict]:
    """What the mapping writes of `frames` (see `run`)."""
    return run(frames, known, then, **extra)[0]


def errors(counters: list[dict]) -> int:
    return sum(int(c["value"]) for c in counters if c["name"] == "processor_error")


@requires_docker
def test_every_report_of_a_vehicle_is_written_in_order_and_an_exact_repeat_once():
    """T-5247 (owner: the complete history of the last five minutes): five reports of one bus in
    one batch, out of order, two of them in the same second and one sent twice, are four writes,
    oldest first; another bus in the same batch is its own write; no drop is an error."""
    reports = [frame(7, "00", tst=tst, long=lon) for tst, lon in
               [("2026-10-21T18:00:01.100Z", 24.941), ("2026-10-21T18:00:02.900Z", 24.943),
                ("2026-10-21T18:00:02.050Z", 24.942), ("2026-10-21T18:00:00.500Z", 24.940),
                ("2026-10-21T18:00:02.900Z", 24.943)]]
    written, counters = run(reports + [frame(8, "00")])
    bus = [(e["location"]["observedAt"], e["location"]["value"]["coordinates"][0])
           for e in written if e["fleetVehicleId"]["value"] == "22-7"]
    assert bus == [("2026-10-21T18:00:00.500Z", 24.940), ("2026-10-21T18:00:01.100Z", 24.941),
                   ("2026-10-21T18:00:02.050Z", 24.942), ("2026-10-21T18:00:02.900Z", 24.943)]
    assert [e["fleetVehicleId"]["value"] for e in written].count("22-8") == 1
    assert errors(counters) == 0, counters


@requires_docker
def test_each_batch_writes_the_vehicle_again():
    """Every second is a new position: the same bus in two batches is written twice, in order."""
    config = {
        "input": {"broker": {"inputs": [{"stdin": {"scanner": {"lines": {}}}}],
                             "batching": {"count": 1000, "period": "200ms"}}},
        "pipeline": pipeline(),
        "cache_resources": short_caches(),
        "rate_limit_resources": yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"],
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "error"},
    }
    runner = subprocess.Popen(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    arrived: queue.Queue[str] = queue.Queue()
    threading.Thread(target=lambda: [arrived.put(line) for line in runner.stdout], daemon=True).start()

    def send(line: str) -> dict:
        runner.stdin.write(line + "\n")
        runner.stdin.flush()
        # Each write is awaited before the next report, so the two never share a batch (T-2992).
        return json.loads(arrived.get(timeout=120))

    try:
        first, second = send(frame(1, "01")), send(frame(1, "02"))
        runner.stdin.close()
        runner.wait(timeout=60)
    finally:
        if runner.poll() is None:
            runner.kill()
    assert runner.returncode == 0, runner.stderr.read()
    assert [e["location"]["observedAt"] for e in (first, second)] == ["2026-09-25T16:01:00.000Z", "2026-09-25T16:02:00.000Z"]
    assert arrived.empty()


@requires_docker
def test_every_mode_is_written_with_its_kind_and_an_unknown_one_is_not():
    """T-5046: every vehicle HSL runs; its U-line and robot buses are buses, a mode the model
    does not name is dropped before it is parsed."""
    modes = [("bus", 22, "2550"), ("ubus", 12, "1500"), ("robot", 18, "9999"), ("tram", 40, "1009"),
             ("metro", 50, "31M1"), ("train", 90, "3001K"), ("ferry", 60, "1019"), ("lightrail", 70, "1015")]
    written, counters = run([frame(v, "01", mode, oper, route) for v, (mode, oper, route) in enumerate(modes, 1)])
    kinds = {e["fleetVehicleId"]["value"]: (e["transportMode"]["value"], e["vehicleType"]["value"]) for e in written}
    assert kinds == {
        "22-1": ("bus", "bus"), "12-2": ("bus", "bus"), "18-3": ("bus", "bus"), "40-4": ("tram", "tram"),
        "50-5": ("metro", "other"), "90-6": ("train", "other"), "60-7": ("ferry", "passengerVessel"),
    }
    assert errors(counters) == 0, counters
    for entity in written:
        assert not open_data.schema_errors(ROOT / "components/context-gateway/seed/helsinki/helsinki.v1.schema.json", entity), entity


@requires_docker
def test_a_malformed_or_oversized_frame_is_dropped_without_an_error():
    """T-5046: the feed is the outside world. A frame that is not JSON, carries no VP object,
    exceeds 4 KiB, or arrives on a topic without a numeric operator and vehicle is dropped, and
    the well-formed frame beside them still lands."""
    bad_topic = topic("bus", 22, 5, "2550").replace("/00022/", "/x/").replace("/0022/", "/x/")
    frames = [
        json.dumps({"topic": topic("bus", 22, 1, "2550"), "raw": '{"VP": {"oper": 22, "veh": 1, "lat": 60.1'}),
        json.dumps({"topic": topic("bus", 22, 2, "2550"), "DOO": {"oper": 22, "veh": 2}}),
        json.dumps({"topic": topic("bus", 22, 3, "2550"), "VP": "a string"}),
        frame(4, "01", desi="x" * 5000),
        json.dumps({"topic": bad_topic, "VP": json.loads(frame(5, "01"))["VP"]}),
        json.dumps({"topic": topic("bus", 22, 6, "2550"), "VP": json.loads(frame(6, "01"))["VP"], "tail": "]"}),
        frame(9, "01"),
    ]
    written, counters = run(frames)
    assert sorted(e["fleetVehicleId"]["value"] for e in written) == ["22-6", "22-9"]
    assert errors(counters) == 0, counters


@requires_docker
def test_a_flood_waits_at_the_safety_valve():
    """T-5046: with no fleet cap left, a feed of more vehicles than HSL runs is held to the
    `hfp_vehicles` rate: 30 vehicles at 10 a second take at least two seconds, and none is lost."""
    started = time.monotonic()
    written = bento([frame(v, "01") for v in range(1, 31)], limit=10)
    took = time.monotonic() - started
    assert len(written) == 30
    # The container's start is in `took` too, so only the floor is asserted.
    assert took >= 2, took


def test_the_safety_valve_admits_two_and_a_half_times_hsls_peak_and_no_more():
    rates = {r["label"]: r["local"] for r in yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"]}
    assert rates["hfp_vehicles"] == {"count": 5000, "interval": "1s"}
    mapping = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    names = [next(iter(p)) for p in mapping]
    # The topic check before the parse, the newest per vehicle, then the valve before the GTFS
    # lookups and the entity.
    assert names[:7] == ["mutation", "mapping", "archive", "mapping", "unarchive", "rate_limit", "mutation"], names
    assert mapping[5]["rate_limit"]["resource"] == "hfp_vehicles"


@requires_docker
def test_a_vehicle_links_its_line_and_next_stop_and_says_how_late_and_full_it_runs():
    urn = "urn:ngsi-ld:{}:hel.fi:helsinki:{}"
    known = (urn.format("GtfsRoute", "1009"), urn.format("GtfsRoute", "3001K"), urn.format("GtfsStop", "1040601"))
    tram, at_end, quiet = bento([
        frame(3, "01", "tram", 40, "1009", dl=-95, occu=100),
        json.dumps({"topic": topic("train", 90, 7, "3001K", "EOL"), "VP": {
            "oper": 90, "veh": 7, "long": 24.94, "lat": 60.17, "route": "3001K",
            "tst": "2026-09-25T16:01:00.000Z", "dl": 30, "occu": 0}}),
        json.dumps({"topic": topic("ferry", 60, 4, "1019"), "VP": {
            "oper": 60, "veh": 4, "long": 24.98, "lat": 60.15, "route": None, "tst": "2026-09-25T16:01:00.000Z"}}),
    ], known, threads=1)
    assert tram["onRoute"]["object"] == urn.format("GtfsRoute", "1009")
    assert tram["nextStop"]["object"] == urn.format("GtfsStop", "1040601")
    # HSL's dl counts ahead of the timetable (-95: 95 s late); delay counts behind it.
    assert tram["delay"]["value"] == 95 and tram["occupancy"]["value"] == 100
    assert tram["transportMode"]["value"] == "tram" and tram["vehicleType"]["value"] == "tram"
    # Past its last stop a train has no next stop.
    assert "nextStop" not in at_end and at_end["delay"]["value"] == -30 and at_end["occupancy"]["value"] == 0
    assert at_end["vehicleType"]["value"] == "other" and at_end["onRoute"]["object"] == urn.format("GtfsRoute", "3001K")
    # A ferry frame without a route names no line.
    assert not {"route", "onRoute"} & set(quiet) and quiet["vehicleType"]["value"] == "passengerVessel"
    for entity in (tram, at_end, quiet):
        assert not open_data.schema_errors(ROOT / "components/context-gateway/seed/helsinki/helsinki.v1.schema.json", entity), entity


@requires_docker
def test_a_line_or_stop_the_space_does_not_hold_is_left_out_and_the_position_still_lands():
    """T-4052: HFP runs lines GTFS does not list (train 3002X3 on 2026-10-12), and the space's view
    refuses a whole Vehicle whose relationship names an entity it does not hold (threat model
    D7). Only a URN the daily GTFS read left in the `gtfs` cache is linked."""
    urn = "urn:ngsi-ld:{}:hel.fi:helsinki:{}"
    known = (urn.format("GtfsRoute", "3001K"), urn.format("GtfsStop", "1040601"))
    frames = [
        json.dumps({"topic": topic("train", 90, v, route, stop), "VP": {
            "oper": 90, "veh": v, "long": 24.94, "lat": 60.17, "route": route, "tst": "2026-10-12T09:30:00.000Z"}})
        for v, route, stop in [(1, "3001K", "1040601"), (2, "3002X3", "1040601"), (3, "3001K", "9999999"), (4, "3002X3", "9999999")]
    ]
    written = {e["fleetVehicleId"]["value"]: e for e in bento(frames, known, threads=1)}
    assert sorted(written) == ["90-1", "90-2", "90-3", "90-4"]
    links = {vehicle: (e.get("onRoute", {}).get("object"), e.get("nextStop", {}).get("object")) for vehicle, e in written.items()}
    assert links == {
        "90-1": (urn.format("GtfsRoute", "3001K"), urn.format("GtfsStop", "1040601")),
        "90-2": (None, urn.format("GtfsStop", "1040601")),
        "90-3": (urn.format("GtfsRoute", "3001K"), None),
        "90-4": (None, None),
    }
    # The line's number and the position land either way.
    assert all(e["route"]["value"] in ("3001K", "3002X3") and e["location"]["value"]["coordinates"] == [24.94, 60.17]
               for e in written.values())
    # Before the first GTFS read of a fresh runner the cache is empty: nothing is linked.
    assert not any({"onRoute", "nextStop"} & set(e) for e in bento(frames, threads=1))


def test_the_source_is_hsls_whole_position_feed_read_over_tls():
    """T-5046: every mode, position reports only, from HSL's broker over WebSocket TLS, read as a
    subscriber (qos 0, no session kept)."""
    spec = yaml.safe_load(
        (ROOT / "components/context-gateway/seed/helsinki/helsinki-datasource-hfp.yaml").read_text()
    )["spec"]
    assert spec["type"] == "mqtt"
    assert spec["mqtt"]["urls"] == ["wss://mqtt.hsl.fi:443/"]
    assert spec["mqtt"]["topics"] == ["/hfp/v2/journey/ongoing/vp/#"]
    assert spec["mqtt"]["qos"] == 0 and spec["mqtt"]["cleanSession"] is True
    assert not {"username", "passwordRef"} & set(spec["mqtt"])


@requires_docker
def test_a_frame_bridged_under_hsl_is_written_as_the_direct_one():
    """T-5883: the dev bridge republishes HSL's topic under `hsl` (an MQTT filter of ours may not
    start with HSL's empty level), and the unchanged mapping reads every level where it was."""
    frames = [frame(v, "01", "tram", 40, "1009") for v in (1, 2)] + [frame(3, "02", "bus", 22, "2550")]
    bridged = [json.dumps({**json.loads(f), "topic": "hsl" + json.loads(f)["topic"]}) for f in frames]
    known = ("urn:ngsi-ld:GtfsRoute:hel.fi:helsinki:1009", "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:1040601")
    direct = bento(frames, known)
    assert len(direct) == 3 and direct[0]["nextStop"]["object"].endswith(":1040601")
    assert bento(bridged, known) == direct
