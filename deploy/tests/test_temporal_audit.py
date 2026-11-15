"""T-4844: every seed pipeline creates history rows that match its measurements.

scripts/temporal-audit.py reads each seed Pipeline's mapping and its space's retention rules and
gives each (pipeline, entity type, attribute) a verdict; CI keeps its JSON report as an artifact.
A seed row that is not ok is fixed in the seed (a retention rule's `record`, an `observedAt` in the
mapping), never listed here as an exception.
"""

import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("temporal_audit", ROOT / "scripts/temporal-audit.py")
audit = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(audit)

VEHICLE = """
root = {
  "id": $id,
  "type": "Vehicle",
  "expiresAt": meta("expires"),
  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [1, 2] }, "observedAt": this.tst },
  "speed": if this.spd.type() != "number" { deleted() } else { {
    "type": "Property", "value": this.spd, "unitCode": "MTS", "observedAt": this.tst
  } },
  # "ghost": { "type": "Property" } in a comment
  "mode": { "type": "Property", "value": "bus" },
  "fixed": { "type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z" },
  "stamped": { "type": "Property", "value": 1, "observedAt": now() },
  $attribute: { "type": "Property", "value": this.v, "unitCode": "GQ", "observedAt": this.at },
  "_helper": { "type": "Property", "value": "{ not a brace }" }
}
"""


def test_the_scan_reads_each_attribute_its_observed_at_and_unit():
    found = audit.scan(VEHICLE, None)
    assert set(found) == {("Vehicle", a) for a in ("location", "speed", "mode", "fixed", "stamped", "$attribute")}
    assert found[("Vehicle", "location")]["observedAt"] == {"mapped"}
    assert found[("Vehicle", "speed")]["unitCode"] == {"MTS"}
    assert found[("Vehicle", "mode")]["observedAt"] == {"received"}
    assert found[("Vehicle", "fixed")]["observedAt"] == {"constant"}
    assert found[("Vehicle", "stamped")]["observedAt"] == {"received"}


def test_a_geojson_type_is_no_entity_type():
    found = audit.scan('root = {"type": "Stop", "location": {"type": "GeoProperty", "value": {"type": "Point"}}, '
                       '"name": {"type": "Property", "value": "x"}}', None)
    assert set(found) == {("Stop", "location"), ("Stop", "name")}


def verdict(**kw):
    base = {"history": "P30D", "record": "every", "observed": {"mapped"}, "measured": False, "per_interval": False}
    return audit.verdict(**(base | kw))


def test_each_verdict_rule():
    assert verdict() == "ok"
    assert verdict(history="off", observed={"constant"}) == "ok"
    assert verdict(observed={"constant"}) == "loses-history"
    assert verdict(record="onChange", per_interval=True) == "loses-history"
    assert verdict(history="forever") == "unbounded-growth"
    assert verdict(observed={"received"}) == "duplicates-rows"
    assert verdict(observed={"mapped", "received"}) == "duplicates-rows"
    assert verdict(observed={"received"}, record="onChange") == "ok"
    assert verdict(observed={"received"}, record="onChange", measured=True) == "stamped-on-receive"


def test_every_seed_row_is_ok_and_the_report_is_written(tmp_path):
    out = tmp_path / "report.json"
    assert audit.main(["--out", str(out)]) == 0
    report = json.loads(out.read_text())
    rows = report["rows"]
    pipelines = {(r["project"], r["pipeline"]) for r in rows}
    assert {p for p, _ in pipelines} >= {"helsinki", "banskabystrica", "bbsk"}
    assert ("helsinki", "hsl-hfp-vehicles") in pipelines
    assert all(r["verdict"] in audit.VERDICTS for r in rows)
    bad = [f'{r["project"]}/{r["pipeline"]} {r["entityType"]}.{r["attribute"]}: {r["verdict"]}' for r in rows if r["verdict"] != "ok"]
    assert bad == []


def test_the_report_holds_no_value_of_the_data():
    """Security (T-4844): the report names attributes and sources, never a sampled value."""
    rows = audit.audit(audit.SEED)
    keys = {k for r in rows for k in r}
    assert keys == {"project", "space", "pipeline", "entityType", "attribute", "record", "observedAt", "unitCode",
                    "datasetId", "history", "lifetime", "verdict"}
    assert {s for r in rows for s in r["observedAt"]} <= {"mapped", "constant", "received"}


def test_each_lifetime_mode_reads_as_pl80_defines_it():
    """T-5019: `from`, `rolling` and `at`, for the whole entity and for some attributes, plus the expiry."""
    spec = {
        "lifetime": [
            {"type": "Vehicle", "entity": {"from": "observedAt", "plus": "PT5M", "notAfter": "PT1H"}},
            {"type": "Sensor", "entity": {"rolling": "PT10M"}},
            {"type": "Event", "entity": {"at": "2027-01-01T00:00:00+01:00"},
             "attributes": [{"names": ["status", "note"], "rolling": "P1D"}, {"names": ["endDate"], "from": "endDate"}]},
        ],
        "expiry": {"after": "14d", "types": ["Vehicle"]},
    }
    assert audit.lifetime_of(spec) == [
        "Event.endDate: from endDate",
        "Event.status,note: rolling P1D",
        "Event: at 2027-01-01T00:00:00+01:00",
        "Sensor: rolling PT10M",
        "Vehicle: from observedAt plus PT5M notAfter PT1H",
        "expiry 14d",
    ]
    assert audit.lifetime_of({}) == []


def test_the_seed_vehicles_report_their_real_lifetime():
    rows = [r for r in audit.audit(audit.SEED) if r["pipeline"] == "hsl-hfp-vehicles"]
    assert rows and all("Vehicle: from observedAt plus PT5M" in r["lifetime"] for r in rows)
    assert not any("set" in r["lifetime"] for r in audit.audit(audit.SEED))
