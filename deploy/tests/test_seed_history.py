"""T-4275: every type a seeded space declares states its history, off or kept for a while.

The owner's defaults (DM-82, ADR-N-046 2.3): a measurement series keeps 30 days of history, every
other type keeps its current state only; the seed never keeps a history forever. A space's model
is its whole list of types (DM-61), so a type without a rule would keep everything it is sent.
"""

import re
from collections import defaultdict
from functools import cache
from pathlib import Path

from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
MEASURED = {
    "AirQualityObserved", "WeatherObserved", "WaterQualityObserved", "KeyPerformanceIndicator",
    "OffStreetParking", "ParkingArea", "BikeHireDockingStation", "TrafficFlowObserved",
}
DURATION = re.compile(r"^P(?!$)(\d+Y)?(\d+M)?(\d+W)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?$")


@cache
def manifests() -> tuple[dict, ...]:
    return tuple(
        doc
        for path, doc in seed_documents()
        if not path.name.endswith(".linkml.yaml") and isinstance(doc, dict) and "kind" in doc
    )


@cache
def spaces() -> dict[str, tuple[dict, set[str]]]:
    classes, found = defaultdict(set), {}
    for doc in manifests():
        ns = doc["metadata"].get("namespace")
        if doc["kind"] == "DataModel":
            ref = doc["spec"].get("contextSpaceRef")
            classes[f"{ns}/{ref['name'] if isinstance(ref, dict) else ref}"] |= set(doc["spec"].get("classes") or [])
        elif doc["kind"] == "ContextSpace":
            found[f"{ns}/{doc['metadata']['name']}"] = doc["spec"]
    return {name: (spec, classes[name]) for name, spec in found.items()}


def test_every_declared_type_has_one_history_rule():
    for name, (spec, types) in spaces().items():
        rules = [rule["entityType"] for rule in spec.get("retention", [])]
        assert len(rules) == len(set(rules)), name
        assert set(rules) == types, f"{name}: {sorted(types ^ set(rules))}"


def test_a_history_is_off_or_a_duration_and_never_forever():
    for name, (spec, _) in spaces().items():
        for rule in spec.get("retention", []):
            history = rule.get("history")
            # a bare `off` is the boolean false to a YAML 1.1 reader: the seed quotes it
            assert history == "off" or (isinstance(history, str) and DURATION.match(history)), (name, rule)
            assert "reason" not in rule, (name, rule)


# A space whose owner set another history for a type, by name (the rest follow the default).
CHOSEN = {
    # The national air quality keeps a week (T-4340, owner 2026-10-16).
    ("slovensko/ovzdusie-sk", "AirQualityObserved"): "P7D",
    # One entity per hub figure, district and period: a history would only repeat it (T-5274).
    ("bratislava/bratislava", "KeyPerformanceIndicator"): "off",
    # Prague's city tables: one entity per figure and month or year, as Bratislava's (T-5428).
    ("praha/praha-mesto", "KeyPerformanceIndicator"): "off",
    # A vehicle keeps five minutes of track (T-5046, owner 2026-10-21; an hour before, T-4624).
    ("helsinki/helsinki", "Vehicle"): "PT5M",
    # The MQTT test space keeps the same five minutes (T-5883).
    ("helsinki/hsl-mqtt-test", "Vehicle"): "PT5M",
    # Žilina's buses keep an hour of track (T-4624) and its counters a month of hours (T-5232).
    ("zilina/zilina-doprava", "Vehicle"): "PT1H",
    ("zilina/zilina-doprava", "TrafficFlowObserved"): "P30D",
    # Helsinki's area statistics keep ten years, the longest a space keeps (DM-82): one row per
    # published year, quarter or month (T-5258).
    ("helsinki/helsinki", "KeyPerformanceIndicator"): "P3650D",
    # Prague's PID vehicles keep an hour of track, as Žilina's (T-4624, T-5268).
    ("praha/praha-mesto", "Vehicle"): "PT1H",
    # Prague's container fill levels keep a week, and the collection points beside them on the one
    # Endpoint praha-trideny-odpad, whose one Policy grants all its types' history or none (T-5270).
    ("praha/praha-mesto", "WasteContainer"): "P7D",
    ("praha/praha-mesto", "WasteContainerIsle"): "P7D",
    # Barcelona's wells are read by hand about once a month and published months late: 400 days keep
    # a year of readings (T-5263, worker decision, owner may override); the public reads the last 30.
    ("barcelona/barcelona", "PhreaticObserved"): "P400D",
    # Barcelona's statistics keep ten years, the longest a space keeps (DM-82): one row per
    # published year or month (T-5266).
    ("barcelona/barcelona", "KeyPerformanceIndicator"): "P3650D",
    # Barcelona's street sections keep a month of traffic states (T-5262, as Žilina's counters).
    ("barcelona/barcelona", "TrafficFlowObserved"): "P30D",
    # Barcelona's charging points keep a month of free ports (T-5262).
    ("barcelona/barcelona", "EVChargingStation"): "P30D",
    # Bratislava's pedestrian counters keep a month of days, as its cyclist counters (T-5273).
    ("bratislava/bratislava", "CrowdFlowObserved"): "P30D",
    # HSL's scheduled departures keep two years of days, the series a daily indicator reads (T-6467).
    ("helsinki/helsinki", "GtfsDailyDepartures"): "P730D",
    # Barcelona's forecast days keep a month of Meteocat's issues (T-5413).
    ("barcelona/barcelona", "WeatherForecast"): "P30D",
}


def chosen(name: str, kind: str) -> str | None:
    # A ŠÚ SR cell is one entity per period, so its history would only repeat it (T-4390).
    return CHOSEN.get((name, kind)) or ("off" if name.startswith("slovensko/susr-") else None)


def test_measurements_keep_thirty_days_and_the_rest_keep_none():
    for name, (spec, _) in spaces().items():
        for rule in spec.get("retention", []):
            want = chosen(name, rule["entityType"]) or ("P30D" if rule["entityType"] in MEASURED else "off")
            assert rule["history"] == want, (name, rule)


def test_the_duration_pattern_refuses_what_is_not_one():
    for bad in ["P", "PT", "30D", "P30", "forever", "P1DT", "off "]:
        assert not DURATION.match(bad), bad
    for good in ["P30D", "PT8H", "P1Y2M", "PT5M", "P2W"]:
        assert DURATION.match(good), good


# T-4372: what the public reads, it reads over time too, as far back as the space keeps it.
TEMPORAL = {"queryTemporal", "retrieveTemporal"}
# An Endpoint bound to one Policy that mixes history-keeping and history-off types: its one Policy
# cannot grant the history of some types and not of others, so it serves no history (T-4372).
# The space's other public Endpoints serve that history.
MIXED_BOUND = {"helsinki/public-transport", "helsinki/public-hub"}
# Public reads of series with at most one row per month and entity, whose whole kept history is
# cheaper to read than a month of a sensor's: they read all of it (T-5258).
WHOLE_HISTORY = {f"helsinki/public-statistics-{theme}" for theme in
                 ("population", "housing", "employment", "income", "education", "services",
                  "welfare", "environment")}
# The heat vulnerability index (one period) and the monthly energy use of the service properties
# are such series too (T-5278).
WHOLE_HISTORY |= {"helsinki/public-heat-vulnerability", "helsinki/public-service-property-energy"}
# Barcelona's statistics themes too (T-5266).
WHOLE_HISTORY |= {f"barcelona/public-statistics-{theme}" for theme in ("population", "housing", "vehicles")}


def public_policies():
    for doc in manifests():
        spec = doc["spec"]
        if doc["kind"] == "Policy" and spec.get("assignee", {}).get("id") == "public":
            ref = spec["contextSpaceRef"]
            space = f"{doc['metadata']['namespace']}/{ref['name'] if isinstance(ref, dict) else ref}"
            yield f"{doc['metadata']['namespace']}/{doc['metadata']['name']}", space, spec


def entries(spec: dict) -> dict[str, dict]:
    """Each type the Policy names, with what it shows of it."""
    return {e["type"]: {k: v for k, v in info.items() if k != "entities"}
            for info in spec.get("information", []) for e in info["entities"]}


def kept(space: str) -> dict[str, int]:
    """The types of the space that keep history, with the days they keep it."""
    spec, _ = spaces()[space]
    return {r["entityType"]: int(r["history"][1:-1]) for r in spec.get("retention", [])
            if r["history"] != "off" and r["history"].endswith("D")}


def test_a_public_temporal_grant_names_only_history_and_is_capped_to_it():
    granting = 0
    for name, space, spec in public_policies():
        if not TEMPORAL & set(spec["operations"]):
            assert "temporalQ" not in spec, name
            continue
        granting += 1
        assert TEMPORAL <= set(spec["operations"]), name
        days = kept(space)
        types = entries(spec)
        assert types and set(types) <= set(days), f"{name}: history off for {sorted(set(types) - set(days))}"
        window = max(days[t] for t in types) if name in WHOLE_HISTORY else min(max(days[t] for t in types), 30)
        assert spec.get("temporalQ") == f"timerel=after;timeAt=P-{window}D", name
    assert granting >= 15


def test_every_public_read_of_a_history_type_reads_its_history_through_the_same_door():
    policies = list(public_policies())
    bound = {}
    for doc in manifests():
        if doc["kind"] == "Endpoint" and doc["spec"].get("policyRef"):
            bound[f"{doc['metadata']['namespace']}/{doc['spec']['policyRef'].rsplit(':', 1)[-1]}"] = True
    for name, space, spec in policies:
        if "retrieveOps" not in spec["operations"]:
            continue
        history = {t: e for t, e in entries(spec).items() if t in kept(space)}
        if not history or name in MIXED_BOUND:
            continue
        # An Endpoint bound to this Policy evaluates it alone; one bound to none, every Policy of the space.
        doors = [spec] if name in bound else [s for _, sp, s in policies if sp == space]
        temporal = {}
        for door in doors:
            if TEMPORAL <= set(door["operations"]):
                temporal.update(entries(door))
        for t, shown in history.items():
            # the same Attributes over time as now, no more
            assert temporal.get(t) == shown, f"{name}: {t} {shown} != {temporal.get(t)}"


def test_the_mixed_bound_policies_still_mix():
    names = {name: (space, spec) for name, space, spec in public_policies()}
    for name in MIXED_BOUND:
        space, spec = names[name]
        days = kept(space)
        assert any(t in days for t in entries(spec)) and any(t not in days for t in entries(spec)), name
        assert not TEMPORAL & set(spec["operations"]), name
