"""T-3904: the helsinki space keeps its transport data under retention and quality rules.

The values are the owner's (DM-81, DM-86, ADR-N-046): a vehicle is gone five minutes after its
last report and keeps five minutes of track (owner, T-5046; an hour before, T-4624); positions lie in what
HSL's feeds cover, speeds up to 160 km/h (the Sm5 commuter trains), and no report is ahead of the clock. Beside the
vehicle only an Alert loses its current state, an hour after its last write, as the net for a writer that sets no end
and for the Fintraffic Alerts written before their mapping gave each one (T-4843); no other type of the seed does.
"""

import csv
from pathlib import Path

import yaml
from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
HELSINKI = SEED / "helsinki"
STOPS = Path(__file__).resolve().parent / "fixtures/helsinki/gtfs/stops.txt"


def spaces() -> dict[str, dict]:
    found = {}
    for path, doc in seed_documents():
        if path.name.endswith(".linkml.yaml"):
            continue
        if isinstance(doc, dict) and doc.get("kind") == "ContextSpace":
            found[f"{doc['metadata']['namespace']}/{doc['metadata']['name']}"] = doc
    return found


def helsinki() -> dict:
    return spaces()["helsinki/helsinki"]["spec"]


def test_a_vehicle_is_kept_five_minutes_after_its_last_report_and_five_minutes_of_its_track():
    (vehicle,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "Vehicle"]
    assert {k: v for k, v in vehicle.items() if k != "attributes"} == {"entityType": "Vehicle", "current": "PT5M", "history": "PT5M"}
    # The three that say what a vehicle is, written without an observedAt, record a change (T-4844).
    assert {name: rule["heartbeat"] for name, rule in vehicle["attributes"].items()
            if name in ("fleetVehicleId", "transportMode", "vehicleType")} == {
        "fleetVehicleId": "PT15M", "transportMode": "PT15M", "vehicleType": "PT15M"}


def test_a_vehicle_keeps_every_report_of_its_last_five_minutes():
    """T-5247 (owner 2026-10-22T17:45Z): the complete history of the last five minutes. What a
    report changes nearly every time records each newer observedAt and no exact repeat; a line,
    next stop or load records its change and a minute's heartbeat, no band holds a change back."""
    (vehicle,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "Vehicle"]
    attributes = vehicle["attributes"]
    model = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    slots = set(model["classes"]["Vehicle"]["slots"]) | {"location"}
    # Every attribute the mapping stamps with the report's observedAt has a rule, each a slot.
    stamped = {"location", "speed", "heading", "delay", "occupancy", "route", "onRoute", "nextStop"}
    assert stamped <= set(attributes) <= slots, set(attributes) - slots
    assert {name: rule for name, rule in attributes.items() if name in stamped} == {
        **{name: {"record": "observed"} for name in ("location", "speed", "heading", "delay")},
        **{name: {"record": "onChange", "heartbeat": "PT1M"} for name in ("occupancy", "route", "onRoute", "nextStop")},
    }


def test_no_vehicle_of_any_space_keeps_more_than_an_hour_of_track():
    """The owner's order (T-4624) holds for every live vehicle feed, not only HSL's."""
    rules = [(name, rule.get("history")) for name, space in spaces().items()
             for rule in space["spec"].get("retention", []) if rule["entityType"] == "Vehicle"]
    assert rules and all(history in ("off", "PT1H", "PT30M", "PT5M") for _, history in rules), rules


def test_no_other_type_loses_its_current_state():
    losing = [(name, rule["entityType"]) for name, space in spaces().items()
              for rule in space["spec"].get("retention", []) if "current" in rule]
    # Barcelona's sections and charging points leave 30 days after the source drops them (T-5262 review).
    assert sorted(losing) == [
        ("barcelona/barcelona", "EVChargingStation"), ("barcelona/barcelona", "OnStreetParking"),
        ("barcelona/barcelona", "TrafficFlowObserved"),
        ("helsinki/helsinki", "Alert"), ("helsinki/helsinki", "Event"),
        ("helsinki/helsinki", "PointOfInterest"), ("helsinki/helsinki", "ServiceRequest"),
        ("helsinki/helsinki", "Vehicle"),
        # The MQTT test space keeps the space helsinki's Vehicle rule (T-5883).
        ("helsinki/hsl-mqtt-test", "Vehicle")]
    # T-5257: the nightly sweep rewrites every event that has not ended, so only a past or
    # withdrawn one reaches its 30 days.
    (event,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "Event"]
    assert event == {"entityType": "Event", "current": "P30D", "history": "off"}
    (unit,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "PointOfInterest"]
    assert unit == {"entityType": "PointOfInterest", "current": "P30D", "history": "off"}
    # The nightly feedback run rewrites 90 days: a report the city unpublishes leaves after 120.
    (report,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "ServiceRequest"]
    assert report == {"entityType": "ServiceRequest", "current": "P120D", "history": "off"}
    (alert,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "Alert"]
    assert alert == {"entityType": "Alert", "current": "PT1H", "history": "off"}


def test_the_alert_net_outlives_every_alert_writers_round():
    """An Alert its feed still lists is rewritten before the hour runs out (DM-83 restarts the clock)."""
    periods = {name: yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())["spec"]["period"]
               for name in ("service-alerts", "traffic-messages")}
    assert periods == {"service-alerts": "60s", "traffic-messages": "10m"}, periods


def test_each_rule_reads_a_slot_of_its_class_and_names_itself_once():
    model = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    rules = helsinki()["quality"]
    assert len({rule["name"] for rule in rules}) == len(rules)
    for rule in rules:
        assert rule["attribute"] in model["classes"][rule["entityType"]]["slots"] + ["location"], rule
    by_name = {rule["name"]: rule for rule in rules}
    assert by_name["vehicle-speed"] == {
        "name": "vehicle-speed", "entityType": "Vehicle", "kind": "range", "attribute": "speed", "min": 0, "max": 44.5,
    }
    assert by_name["vehicle-report-not-ahead"]["maxAhead"] == "PT30S"


def test_every_recorded_stop_lies_in_the_area():
    (area,) = [rule["bbox"] for rule in helsinki()["quality"] if rule["name"] == "stop-in-hsl-area"]
    min_lon, min_lat, max_lon, max_lat = area
    stops = [row for row in csv.DictReader(STOPS.open()) if row["stop_lat"].strip()]
    assert stops
    outside = [row["stop_name"] for row in stops
               if not (min_lon <= float(row["stop_lon"]) <= max_lon and min_lat <= float(row["stop_lat"]) <= max_lat)]
    assert outside == []
    # Rautatientori lies inside, Tampere does not: the box is HSL's, not Finland's.
    assert min_lon <= 24.94 <= max_lon and min_lat <= 60.17 <= max_lat
    assert not (min_lat <= 61.5 <= max_lat)


def test_bike_stations_record_a_change_and_an_hourly_heartbeat_and_no_slot_is_a_count():
    """T-4669 (ADR-N-054 2.2, DM-110): onChange fits levels only, never a quantity per interval."""
    (bikes,) = [rule for rule in helsinki()["retention"] if rule["entityType"] == "BikeHireDockingStation"]
    # T-5058 (DM-132): dateModified is bookkeeping the pipeline stamps on every write, so it keeps no rows.
    assert bikes == {"entityType": "BikeHireDockingStation", "history": "P30D", "record": "onChange", "heartbeat": "PT1H",
                     "attributes": {"dateModified": {"record": "never"}}}
    model = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    assert "dateModified" in model["classes"]["BikeHireDockingStation"]["slots"]
    slots = model["classes"]["BikeHireDockingStation"]["slots"]
    counted = [slot for slot in slots
               if (model.get("slots", {}).get(slot) or {}).get("annotations", {}).get("quantity") == "perInterval"]
    assert counted == []


def test_no_space_keeps_a_history_of_a_refresh_stamp():
    """T-5848: `dateModified` is the pipeline's refresh stamp and a KPI's `updatedAt` the run's own
    clock; each is new on every write, so `record: onChange` kept a row per run (prod: 24k rows an hour
    for Praha's and Bratislava's bike stations). Every rule that names one records never."""
    stamps = {"dateModified": None, "updatedAt": "KeyPerformanceIndicator"}
    kept = [(name, rule["entityType"], attribute, setting)
            for name, space in spaces().items()
            for rule in space["spec"].get("retention", [])
            for attribute, setting in rule.get("attributes", {}).items()
            if attribute in stamps and stamps[attribute] in (None, rule["entityType"])
            and setting != {"record": "never"}]
    assert kept == [], kept
