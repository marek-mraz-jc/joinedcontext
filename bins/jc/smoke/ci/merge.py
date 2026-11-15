#!/usr/bin/env python3
"""merge.py --out <dir> --expect a,b,... [--known <file>] <shard dir>...: the smoke gate's one verdict.

Each shard job of .github/workflows/smoke.yml runs `run all --shard <name>` (gate.sh SMOKE_SHARD) and
uploads its <out>, whose results.json names the shard and its plan (every run name the gate runs).
This folds them into the <out>/results.json and <out>/REPORT.md `run all` writes, the same rows and
the same verdict as one run of every suite:
- a suite split across shards (ui by persona, endpoints by --shard) keeps every shard's rows; an id
  of such a suite in two shards fails (a row would count twice);
- EVERY (logs) runs on each shard, over that shard's jc log and result dirs: rows of one id fold
  into one, failed when any shard failed it, else passed when any passed;
- the known list is applied again over the merged rows (`N not on the known list`, fixed lines);
- a shard of --expect without results.json, a shard not expected, and a run name some shard's plan
  holds that no shard ran each add a failing `shards/...` row: the verdict is then RED.
Answers 0 only when GREEN."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from collections import Counter, defaultdict
from pathlib import Path

SMOKE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SMOKE))
from common import FAIL, PASS, SKIP

# by path: every suite's module is suite.py, and the ci tests share one process with the others
_spec = importlib.util.spec_from_file_location("smoke_all", SMOKE / "all" / "suite.py")
all_suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(all_suite)

EVERY = ("logs",)
RANK = {FAIL: 0, PASS: 1, SKIP: 2}


def run_of(row: dict) -> str:
    return row["id"].split("/", 1)[0]


def fold(rows: list[tuple[str, dict]]) -> dict:
    """One row of an EVERY suite from each shard's row of that id: the worst wins, naming its shards."""
    worst = min(RANK[r["result"]] for _, r in rows)
    named = [(s, r) for s, r in rows if RANK[r["result"]] == worst]
    first = named[0][1]
    reason = "; ".join(f"shard {s}: {r['reason']}" if r.get("reason") else f"shard {s}" for s, r in named)
    return {**first, "reason": reason[:400]}


def merge(shards: dict[str, dict], expect: list[str]) -> tuple[list[dict], dict[str, int]]:
    """Runs (`run all` write's shape) of the shards' rows, plus the seconds each shard took."""
    gaps = [all_suite.row("shards", f"missing:{n}", "", f"shard {n}: results.json", "no results.json", FAIL,
                          "the shard job died, was cancelled or uploaded nothing: its rows are unjudged")
            for n in expect if n not in shards]
    gaps += [all_suite.row("shards", f"unexpected:{n}", "", "a shard of --expect", f"shard {n}", FAIL,
                           "the workflow's matrix and merge --expect disagree") for n in shards if n not in expect]
    rows_of: dict[str, list[tuple[str, dict]]] = defaultdict(list)
    meta: dict[str, list[dict]] = defaultdict(list)
    plan: list[str] = []
    for name, doc in shards.items():
        plan += [p for p in doc.get("plan", []) if p not in plan]
        for s in doc.get("suites", []):
            meta[s["suite"]].append({**s, "shard": name})
        for r in doc.get("checks", []):
            r = {k: v for k, v in r.items() if k != "known"}
            rows_of[run_of(r)].append((name, r))
    gaps += [all_suite.row("shards", f"not-run:{p}", "", f"{p} runs on one shard", "on no shard", FAIL,
                           "the plan holds it and no shard's --only picked it") for p in plan if p not in meta]
    order = [p for p in plan if p in meta] + [s for s in meta if s not in plan]
    runs = []
    for s in order:
        tagged = rows_of.get(s, [])
        if s in EVERY:
            by: dict[tuple[str, int], list[tuple[str, dict]]] = defaultdict(list)
            seen: Counter = Counter()
            for name, r in tagged:
                seen[(name, r["id"])] += 1
                by[(r["id"], seen[(name, r["id"])])].append((name, r))
            rows = [fold(v) for v in by.values()]
        else:
            where: dict[str, set[str]] = defaultdict(set)
            for name, r in tagged:
                where[r["id"]].add(name)
            rows = [r for _, r in tagged]
            gaps += [all_suite.row("shards", f"twice:{i}", "", "one shard", ", ".join(sorted(n)), FAIL,
                                   "a row of a split suite came from two shards")
                     for i, n in where.items() if len(n) > 1]
        secs = [m["seconds"] for m in meta[s]]
        exits = [m["exit"] for m in meta[s]]
        runs.append({"suite": s, "rows": rows, "seconds": max(secs) if s in EVERY else sum(secs),
                     "exit": next((e for e in exits if e not in (0, None)), exits[0]),
                     "shards": sorted(m["shard"] for m in meta[s])})
    if gaps:
        runs.append({"suite": "shards", "rows": gaps, "seconds": 0, "exit": 1, "shards": []})
    return runs, {n: d.get("seconds", 0) for n, d in shards.items()}


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="merge.py", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--expect", required=True, help="comma list of the shard names the matrix runs")
    p.add_argument("--known", help="the gate's known list (ci/known.txt)")
    p.add_argument("dirs", nargs="*", type=Path, help="each shard's out dir (results.json in it)")
    a = p.parse_args(argv)
    expect = [x for x in a.expect.split(",") if x]
    shards: dict[str, dict] = {}
    for d in a.dirs:
        f = d / "results.json"
        if not f.is_file():
            continue
        doc = json.loads(f.read_text())
        shards[doc.get("shard") or d.name] = doc
    runs, secs = merge(shards, expect)
    rows = [r for x in runs for r in x["rows"]]
    fixed = all_suite.mark_known(rows, all_suite.load_known(a.known))
    a.out.mkdir(parents=True, exist_ok=True)
    where = {x["suite"]: x["shards"] for x in runs}

    def report(suite: str) -> str:
        return ", ".join(os.path.relpath(d / suite / "REPORT.md", a.out) for d in a.dirs
                         if (d / suite / "REPORT.md").exists()) or "shards"
    base = next((d.get("base") for d in shards.values()), "")
    note = ("Merged from " + str(len(shards)) + " of " + str(len(expect)) + " shards: "
            + ", ".join(f"{n} ({secs[n]} s: {', '.join(s for s, w in where.items() if n in w)})" for n in sorted(secs))
            + ". Seconds per suite add up its shards; logs runs on every shard (its slowest shown).")
    green = all_suite.write(a.out, base, runs, max(secs.values(), default=0), fixed, note,
                            {"shards": sorted(shards), "expected": expect}, report)
    print((a.out / "REPORT.md").read_text().split("\n## Failures")[0])
    return 0 if green else 1


if __name__ == "__main__":
    sys.exit(main())
