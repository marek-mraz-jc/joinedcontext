#!/usr/bin/env python3
"""The test files one ci.yml `tests` shard runs, balanced by measured time (T-4789).

usage: scripts/ci/test-shard.py <shard> <shards>          prints that shard's files (tests/<name>), one per line
       scripts/ci/test-shard.py --record <file>...         rewrites ci/test-durations.json from the shards'
                                                           `pytest --junitxml` reports (or `--durations=0` logs)

Split by file position (line n in shard n mod 4), the heavy render and container files met in
one shard: shard 4 ran 13.6 min of pytest against 3-5.5 min for the others and passed the job's
15 minutes on three main runs of 2026-10-20. Here each file weighs its seconds in
ci/test-durations.json and goes, heaviest first, to the shard with the least time so far
(longest processing time first); a file the table lacks weighs the median, so a new test file
still lands in exactly one shard. Shard 1 starts with SHARD_ONE_STEPS seconds, its extra steps
(gitleaks over the history, the docs pin, the kyverno apply).

The table went stale within days (T-5551: 73 of 271 files missing, shard 1 at 1,192 s of its 20
minutes against 544 s for shard 2), so every ci.yml shard uploads its junit report and the
`test-durations` job on main prints the table they add up to: commit it when the shards drift.

A file heavier than a quarter of one shard's share goes to every shard, and the conftest hook
`pytest_collection_modifyitems` keeps every count-th of its tests there (JC_TEST_SHARD=<shard>/<count>),
so no one file pins a shard (T-5886: test_smoke_script.py weighed 1,393 s of a 3,300 s share).
"""
import json
import os
import re
import statistics
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / "ci" / "test-durations.json"
# Shard 1's own steps took 120 s of wall in run 40123797109 (kyverno apply 85, temporal audit 17,
# docs pin 11, gitleaks 4, kyverno test 3); the table counts test seconds, and pytest -n auto
# finishes about PARALLEL of them per wall second (run 40149033966: its 10,698 s of test time, the
# table, in 2,824 s of pytest wall over the four shards).
PARALLEL = 3.8
# Run 40589499373 (T-5886): 150 s (kyverno policy verify 99, temporal audit 30, docs pin 12, gitleaks 4).
SHARD_ONE_STEPS = round(150 * PARALLEL)
# A file heavier than 1/SPLIT of one shard's share is split over the shards by its tests
SPLIT = 4
# `12.34s call     test_x.py::test_y[param]` (setup and teardown lines count too)
DURATION = re.compile(r"^(\d+(?:\.\d+)?)s (?:call|setup|teardown)\s+(?:tests/)?(test_[\w]+\.py)::")


def files() -> list[str]:
    """Every test file (JC_TEST_FILES unset or `*`), or the ones JC_TEST_FILES names (tests/<name>,
    space-separated, maybe none): the scoped set of scripts/ci/changes.py, split the same way (T-6152)."""
    chosen = os.environ.get("JC_TEST_FILES")
    if chosen is None or chosen.strip() == "*":
        return all_files()
    return sorted(Path(f).name for f in chosen.split())


def all_files() -> list[str]:
    return sorted(p.name for p in (ROOT / "tests").glob("test_*.py"))


def split(names: list[str], weights: dict[str, float], count: int) -> set[str]:
    """The files each shard runs a count-th of the tests of."""
    share = (sum(weights.get(n, 0.0) for n in names) + SHARD_ONE_STEPS) / count
    return {n for n in names if count > 1 and weights.get(n, 0.0) > share / SPLIT and not grouped(n)}


def grouped(name: str) -> bool:
    """A module under an xdist_group shares one container over its tests, in order: split, every shard
    would start its own container and run tests that lean on state an earlier one left (T-5886)."""
    path = ROOT / "tests" / name
    return path.is_file() and "xdist_group" in path.read_text(encoding="utf-8")


def mine(position: int, shard: int, count: int) -> bool:
    """Whether the test at this collection position of a split file runs in this shard (1-based)."""
    return position % count == shard - 1


def shards(names: list[str], weights: dict[str, float], count: int) -> list[list[str]]:
    known = [weights[n] for n in names if n in weights]
    default = statistics.median(known) if known else 1.0
    load = [0.0] * count
    load[0] = SHARD_ONE_STEPS
    out: list[list[str]] = [[] for _ in range(count)]
    cut = split(names, weights, count)
    for name in sorted(cut):
        for i in range(count):
            out[i].append(name)
            load[i] += weights[name] / count
    for name in sorted((n for n in names if n not in cut), key=lambda n: (-weights.get(n, default), n)):
        least = min(range(count), key=lambda i: (load[i], i))
        out[least].append(name)
        load[least] += weights.get(name, default)
    return [sorted(s) for s in out]


def durations(report: Path) -> dict[str, float]:
    """Seconds per test file in one shard's junit report or `pytest --durations=0` log."""
    totals: dict[str, float] = {}
    if report.suffix == ".xml":
        known = set(all_files())
        for case in ET.parse(report).iter("testcase"):
            # classname is the module path, then the class: tests.test_x.TestY
            name = next((f"{p}.py" for p in case.get("classname", "").split(".") if f"{p}.py" in known), None)
            if name:
                totals[name] = totals.get(name, 0.0) + float(case.get("time") or 0)
        return totals
    for line in report.read_text(encoding="utf-8").splitlines():
        m = DURATION.match(line.strip())
        if m:
            totals[m.group(2)] = totals.get(m.group(2), 0.0) + float(m.group(1))
    return totals


def record(reports: list[Path], old: dict[str, float] | None = None) -> dict[str, float]:
    totals: dict[str, float] = {}
    for report in reports:
        for name, seconds in durations(report).items():
            totals[name] = totals.get(name, 0.0) + seconds
    if not totals:
        raise SystemExit(f"{' '.join(map(str, reports))}: no junit test cases or `pytest --durations=0` lines")
    # A file no report names keeps its old weight (a shard that did not finish); one never measured,
    # or only of tests pytest's log hides (under 5 ms), weighs nothing
    old = old or {}
    return {name: round(totals[name] if name in totals else old.get(name, 0.0), 1) for name in all_files()}


def main(argv: list[str]) -> int:
    if len(argv) >= 2 and argv[0] == "--record":
        table = record([Path(a) for a in argv[1:]], json.loads(TABLE.read_text(encoding="utf-8")))
        TABLE.write_text(json.dumps(table, indent=1) + "\n", encoding="utf-8")
        return 0
    if len(argv) != 2 or not all(a.isdigit() for a in argv) or not 1 <= int(argv[0]) <= int(argv[1]):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    weights = json.loads(TABLE.read_text(encoding="utf-8"))
    # paths from the repo root, where ci.yml runs pytest: a bare name collects nothing (T-4791)
    print("\n".join(f"tests/{n}" for n in shards(files(), weights, int(argv[1]))[int(argv[0]) - 1]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
