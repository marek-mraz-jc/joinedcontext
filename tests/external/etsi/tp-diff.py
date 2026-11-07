#!/usr/bin/env python3
"""Per-TP difference between two ETSI runs of the same broker (T-3549).

Usage: tp-diff.py <pinned tp-status.csv> <latest tp-status.csv> <out.md>

The inputs are dev/etsi-report.py's tp-status.csv (suite, source, test, status,
tags). A TP is keyed by the first word of its test name (the TP id, e.g.
001_01_01), so a retitled TP is still the same TP. Writes the counts of each
run and every TP whose status differs, or that ran in one run only; a missing
input reads as a run with no TPs. Exits 0.
"""
import csv
import sys

STATUSES = ("PASS", "FAIL", "SKIP", "NOT RUN")


def load(path):
    try:
        with open(path, newline="") as fh:
            rows = list(csv.DictReader(fh))
    except FileNotFoundError:
        return None
    return {(r["test"].split() or ["?"])[0]: r["status"] for r in rows}


def counts(run):
    c = {s: 0 for s in STATUSES}
    for s in run.values():
        c[s] = c.get(s, 0) + 1
    return c


def diff(a, b):
    return sorted((tp, a.get(tp, "absent"), b.get(tp, "absent"))
                  for tp in set(a) | set(b) if a.get(tp, "absent") != b.get(tp, "absent"))


def report(a, b):
    lines = ["| run | total | " + " | ".join(s.lower() for s in STATUSES) + " |",
             "|---|---|" + "---|" * len(STATUSES)]
    for name, run in (("pinned (a)", a), ("latest (b)", b)):
        if run is None:
            lines.append(f"| {name} | no results | " + " | ".join("" for _ in STATUSES) + " |")
            continue
        c = counts(run)
        lines.append(f"| {name} | {len(run)} | " + " | ".join(str(c[s]) for s in STATUSES) + " |")
    d = diff(a or {}, b or {})
    lines += ["", f"TPs that differ from (a): {len(d)}", ""]
    if d:
        lines += ["| TP | (a) | (b) |", "|---|---|---|"] + [f"| {tp} | {x} | {y} |" for tp, x, y in d]
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    text = report(load(sys.argv[1]), load(sys.argv[2]))
    with open(sys.argv[3], "w") as fh:
        fh.write(text)
    print(text)
