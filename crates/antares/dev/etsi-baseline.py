#!/usr/bin/env python3
"""Per-TP ETSI gate (T-3466): every cell runs exactly the TPs the committed
baseline names, and each one passes.

A cell's own gate (dev/etsi-report.py) is 100 % green over the TPs that ran.
What it cannot see is a TP that stopped running — a tag excluded, a file
that no longer parses, a suite dropped — because a smaller green run is
still green. This gate compares, per TP, against dev/etsi-baseline.json.

Usage:
  etsi-baseline.py check <cells-dir>   every ETSI-cell-<store>/tp-status.csv
                                       (stores from STORES, as the summary)
  etsi-baseline.py write <results-dir> regenerate the baseline from one
                                       run's tp-status.csv (a green run, or a
                                       `robot --dryrun` folded by etsi-report)

  etsi-baseline.py compare <vanilla> <full>
                                       the same TPs ran in both runs with the
                                       same verdict (T-3675: the IOP suite on
                                       the all-features build answers as the
                                       vanilla build of the same commit does)

check exits nonzero naming every TP that failed or did not run; a TP that
ran and is not in the baseline prints "ratchet: update baseline" and passes.
A cell named in MQTT_OFF runs with MQTT=0 (dev/etsi-pipeline.sh forces it for
WASM), so the baseline's MQTT-tagged TPs (robot `--exclude '*mqtt*'`) may be
absent there. BASELINE overrides the baseline path (the selftest uses it).

EXPECTED=<file> (the policy cells of image-jc.yml, T-4855) names the TPs a
cell must fail, one `<TP id>  <reason>` per line (`#` comments), the id being
the first word of the test name: such a TP failing is green, and passing is
red (the refusal it stands for did not happen). Every other TP is checked as
above.
"""
import csv
import glob
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
BASELINE = os.environ.get("BASELINE") or f"{HERE}/etsi-baseline.json"
STORES = (os.environ.get("STORES") or "file postgres timescale").split()
MQTT_OFF = (os.environ.get("MQTT_OFF") or "").split()


def rows_of(d):
    """The rows of the tp-status.csv under `d`, or None."""
    hits = glob.glob(f"{d}/**/tp-status.csv", recursive=True)
    if not hits:
        return None
    return list(csv.DictReader(open(hits[0], newline="")))


def key(r):
    return f"{r['source']}::{r['test']}"


def run_of(d):
    """{"<file>::<name>": status} from the tp-status.csv under `d`, or None."""
    rows = rows_of(d)
    return None if rows is None else {key(r): r["status"] for r in rows}


def expected_failures():
    """{TP id: reason} from the EXPECTED file, or {} without one."""
    path = os.environ.get("EXPECTED")
    if not path:
        return {}
    out = {}
    for line in open(path):
        tp, _, why = line.strip().partition(" ")
        if tp and not tp.startswith("#"):
            if not why.strip():
                raise SystemExit(f"{path}: {tp} has no reason")
            out[tp] = why.strip()
    return out


def tp_id(k):
    return (k.split("::", 1)[-1].split() or ["?"])[0]


def check(cells_dir):
    baseline = json.load(open(BASELINE))
    expected = expected_failures()
    red = []
    for st in STORES:
        base = set(baseline["tps"])
        if st in MQTT_OFF:
            base -= set(baseline.get("mqtt_tps", []))
        ran = run_of(f"{cells_dir}/ETSI-cell-{st}")
        if ran is None:
            red.append(f"{st}: no tp-status.csv")
            continue
        missing = sorted(base - ran.keys())
        failed = sorted(k for k, v in ran.items() if v != "PASS" and tp_id(k) not in expected)
        refused = {tp_id(k) for k, v in ran.items() if v != "PASS"} & expected.keys()
        new = sorted(k for k in ran.keys() - base if ran[k] == "PASS")
        for k in missing:
            red.append(f"{st}: did not run: {k}")
        for k in failed:
            red.append(f"{st}: {ran[k]}: {k}")
        for tp in sorted(expected.keys() - refused):
            red.append(f"{st}: expected to fail and did not: {tp} ({expected[tp]})")
        for k in new:
            print(f"{st}: ratchet: update baseline: {k} passes and is not in it")
        print(f"{st}: {len(ran)} ran, {len(missing)} missing, {len(failed)} not passing, {len(new)} new")
    for line in red:
        print(line)
    return 1 if red else 0


def compare(vanilla_dir, full_dir):
    """Every TP whose verdict differs between the two runs, or that ran in
    one only, is named and fails. An empty run fails too: two runs that
    executed nothing agree, and say nothing."""
    vanilla, full = run_of(vanilla_dir), run_of(full_dir)
    red = [f"{d}: no TP ran (tp-status.csv missing or empty)" for d, r in ((vanilla_dir, vanilla), (full_dir, full)) if not r]
    if not red:
        for k in sorted(vanilla.keys() | full.keys()):
            a, b = vanilla.get(k, "did not run"), full.get(k, "did not run")
            if a != b:
                red.append(f"vanilla {a}, full {b}: {k}")
        print(f"{len(vanilla)} TPs vanilla, {len(full)} full, {len(red)} differ")
    for line in red:
        print(line)
    return 1 if red else 0


def write(results_dir):
    rows = rows_of(results_dir)
    if not rows:
        sys.exit(f"no TPs in {results_dir}/**/tp-status.csv")
    ran = {key(r) for r in rows}
    mqtt = {key(r) for r in rows if "mqtt" in (r.get("tags") or "").lower()}

    def git(*args):
        r = subprocess.run(["git", *args], cwd=os.path.dirname(HERE), capture_output=True, text=True)
        return r.stdout.strip() or None

    baseline = {
        "suite": {
            "path": "ngsi-ld-test-suite",
            "upstream": "https://forge.etsi.org/rep/cim/ngsi-ld-test-suite",
            "spec": "ETSI GS CIM 009 V1.9.1",
            # the vendored tree's own hash: changes exactly when a TP file does
            "tree": git("rev-parse", "HEAD:ngsi-ld-test-suite"),
            "last_commit": git("log", "-1", "--format=%H", "--", "ngsi-ld-test-suite"),
        },
        "build": "default features (the image the etsi-matrix cells run)",
        "excluded_tags": ["config_no_temporal"],
        "count": len(ran),
        "tps": sorted(ran),
        # absent where a cell runs with MQTT=0 (MQTT_OFF)
        "mqtt_tps": sorted(mqtt),
    }
    with open(BASELINE, "w") as fh:
        json.dump(baseline, fh, indent=1, ensure_ascii=False)
        fh.write("\n")
    print(f"wrote {BASELINE}: {len(ran)} TPs")
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["compare"] and len(sys.argv) == 4:
        sys.exit(compare(sys.argv[2], sys.argv[3]))
    if len(sys.argv) != 3 or sys.argv[1] not in ("check", "write"):
        sys.exit(__doc__)
    sys.exit((check if sys.argv[1] == "check" else write)(sys.argv[2]))
