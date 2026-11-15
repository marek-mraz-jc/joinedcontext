"""T-5056, DM-132: history is opt-in (the broker runs ANTARES_TEMPORAL_RECORD=ruled), so a seeded retention rule says what it
keeps: a duration, `forever` or `off`. A rule that names a type without `history` would record nothing and read as if it
kept something; a type no rule names records nothing at all.
"""

import re
from pathlib import Path

from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
DURATION = re.compile(
    r"^P(?!$)(\d+Y)?(\d+M)?(\d+W)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+(\.\d+)?S)?)?$"
)


def rules():
    for path, doc in seed_documents():
        if path.name.endswith(".linkml.yaml"):
            continue
        if isinstance(doc, dict) and doc.get("kind") == "ContextSpace":
            for rule in doc.get("spec", {}).get("retention") or []:
                yield path.relative_to(SEED), rule


def test_every_seeded_retention_rule_names_its_history():
    seen = list(rules())
    assert seen, "no seeded retention rule found"
    unset = [f"{p}: {r.get('entityType')}" for p, r in seen if "history" not in r]
    assert not unset, f"rules without history (a duration, forever or off): {unset}"
    bad = [
        f"{p}: {r['entityType']}={r['history']!r}"
        for p, r in seen
        if r["history"] not in ("off", "forever")
        and not DURATION.match(str(r["history"]))
    ]
    assert not bad, f"history is a duration such as P30D, forever or off: {bad}"


def test_the_measured_types_keep_their_history():
    kept = {(p.parts[0], r["entityType"]) for p, r in rules() if r["history"] != "off"}
    for space, entity_type in [
        ("helsinki", "AirQualityObserved"),
        ("helsinki", "WeatherObserved"),
        ("helsinki", "BikeHireDockingStation"),
        ("helsinki", "Vehicle"),
        ("helsinki", "KeyPerformanceIndicator"),
    ]:
        assert (space, entity_type) in kept, f"{space} {entity_type} lost its history"


def test_a_late_write_goes_to_history_and_never_replaces_the_current_value():
    """T-5326, DM-106: the Portal's backfill and correction of a past measurement are plain writes."""
    values = (SEED.parents[2] / "components/context-broker/values/broker/base-values.yaml.gotmpl").read_text()
    assert 'ANTARES_HISTORY_BACKFILL: "on"' in values
