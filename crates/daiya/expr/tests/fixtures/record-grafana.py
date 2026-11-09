#!/usr/bin/env python3
"""Record Grafana 12.4.14's server-side expressions over one TestData frame.

Each scenario posts query A (TestData `csv_content`: a time column and two value columns, with a
null, a NaN and an infinity) and one `__expr__` node to the reference's /api/ds/query, and keeps
Grafana's answer. tests/grafana_parity.rs feeds Grafana's own A frame to daiya's pipeline and
compares every node's frames with Grafana's.

    GRAFANA=http://localhost:42161 python3 record-grafana.py   # writes grafana-expressions.json
"""
import base64
import json
import os
import urllib.request

GRAFANA = os.environ.get("GRAFANA", "http://localhost:42161")
HERE = os.path.dirname(os.path.abspath(__file__))
CSV = "\n".join(
    ["time,a,b"]
    + [
        f"{t},{a},{b}"
        for t, a, b in zip(
            range(1000, 9000, 1000),
            ["1", "", "NaN", "4", "10", "5", "+Inf", "2"],
            ["3", "3", "-2", "3", "", "0", "3", "7"],
        )
    ]
)
FROM, TO = 1000, 8000
A = {
    "refId": "A",
    "datasource": {"type": "grafana-testdata-datasource", "uid": "testdata"},
    "scenarioId": "csv_content",
    "csvContent": CSV,
}
EXPR = {"type": "__expr__", "uid": "__expr__"}


def node(ref, **spec):
    return {"refId": ref, "datasource": EXPR, **spec}


SCENARIOS = {}
for i, expression in enumerate(
    [
        "$A * 2",
        "$A + $A",
        "$A - 1.5",
        "$A / 0",
        "abs($A - 5)",
        "log($A)",
        "$A > 3",
        "$A >= 4 && $A < 10",
        "!($A == 4)",
        "-$A",
        "$A % 3",
        "$A ** 2",
        "is_nan($A)",
        "is_null($A)",
        "is_inf($A)",
        "is_number($A)",
        "round($A / 3)",
        "ceil($A / 3)",
        "floor($A / 3)",
    ]
):
    SCENARIOS[f"math-{i:02}"] = [node("B", type="math", expression=expression)]
for reducer in ["sum", "mean", "min", "max", "count", "last", "median"]:
    for mode, settings in [("strict", {}), ("dropNN", {"mode": "dropNN"}), ("replaceNN", {"mode": "replaceNN", "replaceWithValue": -1})]:
        SCENARIOS[f"reduce-{reducer}-{mode}"] = [node("B", type="reduce", expression="A", reducer=reducer, settings=settings)]
for down in ["mean", "min", "max", "sum", "last"]:
    for up in ["fillna", "pad", "backfilling"]:
        for window in ["2s", "500ms"]:
            SCENARIOS[f"resample-{window}-{down}-{up}"] = [
                node("B", type="resample", expression="A", window=window, downsampler=down, upsampler=up)
            ]
for evaluator, params in [("gt", [3]), ("lt", [3]), ("within_range", [2, 5]), ("outside_range", [2, 5])]:
    SCENARIOS[f"threshold-{evaluator}"] = [
        node("B", type="reduce", expression="A", reducer="last", settings={"mode": "dropNN"}),
        node("C", type="threshold", expression="B", conditions=[{"evaluator": {"type": evaluator, "params": params}}]),
    ]


def ask(queries):
    body = json.dumps({"from": str(FROM), "to": str(TO), "queries": queries}).encode()
    request = urllib.request.Request(
        f"{GRAFANA}/api/ds/query",
        data=body,
        headers={
            "Content-Type": "application/json",
            "Authorization": "Basic " + base64.b64encode(b"admin:admin").decode(),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as answer:
            return answer.status, json.load(answer)
    except urllib.error.HTTPError as refused:
        return refused.code, json.load(refused)


def frames(result):
    """Grafana's DataFrameJSON as plain columns, NaN and infinities as the strings Grafana's entities name."""
    out = []
    for frame in result.get("frames", []):
        schema, data = frame["schema"], frame.get("data", {"values": []})
        columns = []
        for at, field in enumerate(schema["fields"]):
            values = list(data["values"][at]) if at < len(data["values"]) else []
            entities = (data.get("entities") or [None] * len(schema["fields"]))[at] or {}
            for word, marks in (("NaN", "NaN"), ("+Inf", "Inf"), ("-Inf", "NegInf")):
                for index in entities.get(marks, []):
                    values[index] = word
            columns.append({"name": field["name"], "type": field["type"], "labels": field.get("labels", {}), "values": values})
        out.append({"name": schema.get("name", ""), "fields": columns})
    return out


recorded = {"grafana": "12.4.14", "from": FROM, "to": TO, "a": A, "scenarios": {}}
for name, nodes in SCENARIOS.items():
    status, answer = ask([A, *nodes])
    results = answer.get("results", {})
    recorded["scenarios"][name] = {
        "nodes": nodes,
        "status": status,
        "results": {ref: {"error": r.get("error"), "frames": frames(r)} for ref, r in results.items()},
    }
with open(os.path.join(HERE, "grafana-expressions.json"), "w") as out:
    json.dump(recorded, out, indent=1)
    out.write("\n")
print(f"{len(SCENARIOS)} scenarios")
