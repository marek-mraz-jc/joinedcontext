#!/usr/bin/env python3
"""The perf gate (T-4853): one run's numbers against the committed baseline.

    gate.py bench  <BENCH_JSON>  ci/perf-baseline/bench-core.json [--write]
    gate.py shapes <shapes.md>   ci/perf-baseline/shapes.json --machine ccx33 [--write]
    gate.py selftest

bench   the core_paths bench (crates/antares-sql/benches/core_paths.rs). A
        case is read as its p50 over the `calibrate` p50 measured right
        before it, so a slower runner moves both; a ratio more than THRESHOLD above the
        baseline's is a regression. Baselines are kept per CPU model.
shapes  dev/perf/shapes.sh's table. A row whose p99 rose, or whose req/s
        fell, by more than THRESHOLD is a regression. Baselines are kept per
        machine (the perf-weekly instance type).

A CPU or machine with no baseline yet is reported and not gated: the run
prints the entry to commit. A baseline row the run no longer produces is a
regression (a shape that stopped running cannot hide). --write folds the
run into the baseline: the reviewed way to accept a change.

Prints a Markdown table; exits 1 on a regression, 2 on unreadable input.
"""

import json
import re
import sys
from pathlib import Path

THRESHOLD = 0.15


def cpu_model():
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return "unknown"


def bench_rows(results):
    cases = results["cases"]
    rows = {}
    for name, c in cases.items():
        if name == "calibrate":
            continue
        # the yardstick measured right before the case, else the run's own
        yardstick = c.get("calibrate_p50_ns") or cases["calibrate"]["p50_ns"]
        if yardstick <= 0:
            raise ValueError(f"{name}: calibrate p50 is zero")
        rows[name] = {"ratio": round(c["p50_ns"] / yardstick, 4), "p50_ns": c["p50_ns"], "p99_ns": c["p99_ns"]}
    return rows


SHAPE_ROW = re.compile(r"^\|\s*(\w+)\s*\|\s*([\w-]+)\s*\|\s*(c\d+)\s*\|\s*([\d.]+)\s*\|\s*([\d.]+) ms\s*\|$")


def shape_rows(text):
    rows = {}
    for line in text.splitlines():
        m = SHAPE_ROW.match(line.strip())
        if m:
            store, shape, conc, rps, p99 = m.groups()
            rows[f"{store} {shape} {conc}"] = {"rps": float(rps), "p99_ms": float(p99)}
    if not rows:
        raise ValueError("no shape rows in the table")
    return rows


def compare(kind, rows, base):
    """(table lines, regressions) for `rows` against `base` (None: not gated)."""
    lines = ["| row | baseline | this run | change | verdict |", "|---|---|---|---|---|"]
    bad = 0
    names = sorted(set(rows) | set(base or {}))
    for name in names:
        now, was = rows.get(name), (base or {}).get(name)
        if now is None:
            lines.append(f"| {name} | present | missing | | regression |")
            bad += 1
            continue
        if was is None:
            shown = now["ratio"] if kind == "bench" else f"{now['rps']:.0f} req/s, p99 {now['p99_ms']} ms"
            lines.append(f"| {name} | none | {shown} | | {'not gated' if base is None else 'new'} |")
            continue
        if kind == "bench":
            change = now["ratio"] / was["ratio"] - 1
            worse = change > THRESHOLD
            lines.append(f"| {name} | {was['ratio']} | {now['ratio']} | {change:+.1%} | {'regression' if worse else 'ok'} |")
        else:
            p99 = now["p99_ms"] / was["p99_ms"] - 1
            rps = now["rps"] / was["rps"] - 1
            worse = p99 > THRESHOLD or rps < -THRESHOLD
            lines.append(
                f"| {name} | {was['rps']:.0f} req/s, p99 {was['p99_ms']} ms | {now['rps']:.0f} req/s, "
                f"p99 {now['p99_ms']} ms | req/s {rps:+.1%}, p99 {p99:+.1%} | {'regression' if worse else 'ok'} |"
            )
        bad += worse
    return lines, bad


def run(kind, results_path, baseline_path, key, write):
    text = Path(results_path).read_text()
    rows = bench_rows(json.loads(text)) if kind == "bench" else shape_rows(text)
    bp = Path(baseline_path)
    baseline = json.loads(bp.read_text()) if bp.exists() else {}
    keyed = baseline.setdefault("cpus" if kind == "bench" else "machines", {})
    base = keyed.get(key, {}).get("rows")
    lines, bad = compare(kind, rows, base)
    print(f"perf gate ({kind}, {key}, threshold {THRESHOLD:.0%})\n")
    print("\n".join(lines))
    if write:
        keyed[key] = {"rows": rows}
        bp.parent.mkdir(parents=True, exist_ok=True)
        bp.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n")
        print(f"\nbaseline for {key} written to {bp}")
        return 0
    if base is None:
        print(f"\nno baseline for {key}: not gated; commit it with --write")
        return 0
    if bad:
        print(f"\n{bad} regression(s) past {THRESHOLD:.0%}")
    return 1 if bad else 0


def selftest():
    calib = {"calibrate": {"p50_ns": 100, "p99_ns": 120}}
    rows = bench_rows({"cases": {**calib, "a": {"p50_ns": 200, "p99_ns": 300}}})
    assert rows["a"]["ratio"] == 2.0
    # a runner twice as slow moves the case and the yardstick together
    slow = bench_rows({"cases": {"calibrate": {"p50_ns": 200, "p99_ns": 240}, "a": {"p50_ns": 400, "p99_ns": 600}}})
    assert compare("bench", slow, rows)[1] == 0
    # the deliberate slowdown: the case 20 % slower on the same runner
    slower = bench_rows({"cases": {**calib, "a": {"p50_ns": 240, "p99_ns": 300}}})
    assert compare("bench", slower, rows)[1] == 1
    assert compare("bench", {}, rows)[1] == 1, "a case that stopped running is a regression"
    assert compare("bench", rows, None)[1] == 0, "no baseline: not gated"
    table = "| store | shape | concurrency | req/s | p99 |\n|---|---|---|---|---|\n" \
            "| postgres | query-q | c50 | 916 | 73.18 ms |\n| full | view | c50 | 3000 | 20.00 ms |\n"
    base = shape_rows(table)
    assert set(base) == {"postgres query-q c50", "full view c50"}
    assert compare("shapes", base, base)[1] == 0
    p99_up = shape_rows(table.replace("73.18 ms", "85.00 ms"))
    assert compare("shapes", p99_up, base)[1] == 1, "p99 +16 % is a regression"
    rps_down = shape_rows(table.replace("| 916 |", "| 770 |"))
    assert compare("shapes", rps_down, base)[1] == 1, "req/s -16 % is a regression"
    within = shape_rows(table.replace("| 916 |", "| 800 |").replace("73.18 ms", "80.00 ms"))
    assert compare("shapes", within, base)[1] == 0
    try:
        shape_rows("no table")
        raise AssertionError("an empty table must not pass")
    except ValueError:
        pass
    print("gate.py selftest: ok")
    return 0


def main(argv):
    if argv[1:2] == ["selftest"]:
        return selftest()
    args = [a for a in argv[1:] if not a.startswith("--")]
    write = "--write" in argv
    if len(args) < 3 or args[0] not in ("bench", "shapes"):
        print(__doc__, file=sys.stderr)
        return 2
    if args[0] == "shapes":
        if "--machine" not in argv or argv.index("--machine") + 1 >= len(argv):
            print("shapes needs --machine <instance type>", file=sys.stderr)
            return 2
        key = argv[argv.index("--machine") + 1]
        args.remove(key)
    else:
        key = cpu_model()
    kind, results, baseline = args[:3]
    try:
        return run(kind, results, baseline, key, write)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as e:
        print(f"perf gate: unreadable input: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
