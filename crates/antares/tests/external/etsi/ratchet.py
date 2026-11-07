#!/usr/bin/env python3
"""ETSI ratchet (T-4855): a run fails exactly the TPs a list names, or fewer.

Usage: ratchet.py <list> <tp-status.csv> [--partial]

<list> holds one TP per line, `<TP id>  <triage>`: the id is the first word
of the test name (as tp-diff.py keys it), the triage the bucket, clause and
report that keep it failing (docs/conformance/final-validation.md 4.1). A
line without a triage, a TP listed twice, and `#` comments are refused or
skipped as written. The run is dev/etsi-report.py's tp-status.csv.

Red (exit 1) when:
- a TP that is not listed does not pass (a regression);
- a listed TP passes (fixed: take it off the list, so it cannot regress
  unseen);
- a listed TP did not run, unless --partial (a smoke run of some suites);
- the run holds no TP at all.
"""
import csv
import sys


def load_list(path):
    """{TP id: triage} from the list file; raises ValueError on a bad line."""
    out = {}
    with open(path) as fh:
        for n, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            tp, _, why = line.partition(" ")
            if not why.strip():
                raise ValueError(f"{path}:{n}: {tp} has no triage line")
            if tp in out:
                raise ValueError(f"{path}:{n}: {tp} is listed twice")
            out[tp] = why.strip()
    return out


def load_run(path):
    """{TP id: status} from a tp-status.csv; an absent file is an empty run."""
    try:
        with open(path, newline="") as fh:
            return {(r["test"].split() or ["?"])[0]: r["status"] for r in csv.DictReader(fh)}
    except FileNotFoundError:
        return {}


def check(listed, run, partial=False):
    """The red lines of `run` against `listed`, and a one-line count."""
    if not run:
        return ["no TP ran (tp-status.csv missing or empty)"], "0 TPs"
    red = []
    failing = {tp for tp, s in run.items() if s != "PASS"}
    for tp in sorted(failing - listed.keys()):
        red.append(f"regression: {tp} {run[tp]} and is not on the list")
    for tp in sorted(listed.keys() & run.keys() - failing):
        red.append(f"fixed: {tp} passes; take it off the list")
    if not partial:
        for tp in sorted(listed.keys() - run.keys()):
            red.append(f"did not run: {tp} is on the list; take it off if the suite dropped it")
    summary = f"{len(run)} TPs, {len(failing)} not passing, {len(listed)} listed"
    return red, summary


def main(argv):
    args = [a for a in argv if a != "--partial"]
    if len(args) != 2:
        sys.exit(__doc__)
    try:
        listed = load_list(args[0])
    except ValueError as e:
        print(e)
        return 1
    red, summary = check(listed, load_run(args[1]), partial="--partial" in argv)
    print(summary)
    for line in red:
        print(line)
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
