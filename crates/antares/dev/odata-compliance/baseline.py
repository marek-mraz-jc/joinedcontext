#!/usr/bin/env python3
"""The regression gate over the NLstn OData compliance suite's verbose output (T-3714).

    dev/odata-compliance/baseline.py write RUN.txt BASELINE.json   record a run as the baseline
    dev/odata-compliance/baseline.py check RUN.txt BASELINE.json   red when a baseline pass is lost
    dev/odata-compliance/baseline.py --self-test

A test is `[V4] 2.1_conformance :: Service supports GET on single entities (MUST)`, its status
PASS, FAIL or SKIP. `check` fails on any test that passed in the baseline and does not pass
now (failed, skipped or gone), never on a known failure; a new pass is listed so the baseline
can be raised. Both print the per-feature table of the run as Markdown for the job summary.
"""

import json
import re
import sys

RUNNING = re.compile(r"^Running: (\[[^\]]+\] \S+)")
RESULT = re.compile(r"^[✓✗⊘] (PASS|FAIL|SKIP): (.+?)\s*$")
FEATURE = re.compile(r"^\s{2}(\S.*?)\s{2,}(Minimal|Intermediate|Advanced)\s+(\d+)\s+(\d+)\s+(\d+)\s+\S+ (?:PASS|FAIL|SKIP) \[([^\]]+)\]")


def parse(text):
    """`({test: status}, [(feature, level, version, passed, failed, skipped)])` of one run."""
    results, features, suite = {}, [], None
    for line in text.splitlines():
        if m := RUNNING.match(line):
            suite = m.group(1)
        elif (m := RESULT.match(line)) and suite:
            key, n = f"{suite} :: {m.group(2)}", 2
            while key in results:  # a suite may name two tests alike
                key, n = f"{suite} :: {m.group(2)} #{n}", n + 1
            results[key] = m.group(1)
        elif m := FEATURE.match(line):
            features.append((m.group(1), m.group(2), m.group(6), *map(int, m.group(3, 4, 5))))
    return results, features


def table(features):
    rows = ["| Feature | Level | Version | Passed | Failed | Skipped |", "|---|---|---|---:|---:|---:|"]
    rows += [f"| {f} | {lv} | {v} | {p} | {x} | {s} |" for f, lv, v, p, x, s in features]
    by_level = {}
    for _, lv, _, p, x, s in features:
        t = by_level.setdefault(lv, [0, 0, 0])
        t[0], t[1], t[2] = t[0] + p, t[1] + x, t[2] + s
    rows += ["", "| Level | Passed | Failed | Skipped | Pass rate (of run) |", "|---|---:|---:|---:|---:|"]
    for lv in ("Minimal", "Intermediate", "Advanced"):
        p, x, s = by_level.get(lv, [0, 0, 0])
        rate = f"{100 * p / (p + x):.1f} %" if p + x else "-"
        rows.append(f"| {lv} | {p} | {x} | {s} | {rate} |")
    return "\n".join(rows)


def counts(results):
    return {s: sum(1 for v in results.values() if v == s) for s in ("PASS", "FAIL", "SKIP")}


def lost(baseline, results):
    """Tests the baseline passes and the run does not, sorted."""
    return sorted(k for k, v in baseline.items() if v == "PASS" and results.get(k) != "PASS")


def gained(baseline, results):
    return sorted(k for k, v in results.items() if v == "PASS" and baseline.get(k) != "PASS")


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 3 or argv[0] not in ("write", "check"):
        sys.exit(__doc__)
    mode, run, path = argv
    with open(run, encoding="utf-8") as f:
        results, features = parse(f.read())
    if not results:
        sys.exit(f"{run}: no test result: was the suite run with -verbose?")
    print(table(features))
    if mode == "write":
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"counts": counts(results), "results": results}, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"\nbaseline written: {counts(results)}")
        return 0
    with open(path, encoding="utf-8") as f:
        baseline = json.load(f)["results"]
    down, up = lost(baseline, results), gained(baseline, results)
    print(f"\nrun {counts(results)}, baseline {counts(baseline)}")
    if up:
        print(f"\n{len(up)} new pass(es), raise the baseline:\n" + "\n".join(f"- {k}" for k in up))
    if down:
        print(f"\n{len(down)} baseline pass(es) lost:\n" + "\n".join(f"- {k}" for k in down))
        return 1
    return 0


SAMPLE = """Running: [V4] 2.1_conformance
✓ PASS: Service returns service document (MUST)
✗ FAIL: Service supports GET on single entities (MUST)
✓ PASS: Twice
✓ PASS: Twice
Running: [V4] 11.2.6.6_query_search
⊘ SKIP: Search is not supported
  Feature                          Level           Passed  Failed  Skipped  Status
  Metadata                         Minimal             16       0       0  ✓ PASS [4.0]
  Counting                         Intermediate         0       9       0  ✗ FAIL [4.0]
  Batch                            Advanced             0       0      17  ⊘ SKIP [4.01]
"""


def self_test():
    results, features = parse(SAMPLE)
    assert results == {
        "[V4] 2.1_conformance :: Service returns service document (MUST)": "PASS",
        "[V4] 2.1_conformance :: Service supports GET on single entities (MUST)": "FAIL",
        "[V4] 2.1_conformance :: Twice": "PASS",
        "[V4] 2.1_conformance :: Twice #2": "PASS",
        "[V4] 11.2.6.6_query_search :: Search is not supported": "SKIP",
    }, results
    assert features[0] == ("Metadata", "Minimal", "4.0", 16, 0, 0), features
    assert features[2] == ("Batch", "Advanced", "4.01", 0, 0, 17), features
    assert "| Minimal | 16 | 0 | 0 | 100.0 % |" in table(features)
    assert "| Intermediate | 0 | 9 | 0 | 0.0 % |" in table(features)
    now = dict(results)
    now["[V4] 2.1_conformance :: Twice #2"] = "FAIL"
    del now["[V4] 2.1_conformance :: Service returns service document (MUST)"]
    now["[V4] 2.1_conformance :: Service supports GET on single entities (MUST)"] = "PASS"
    assert lost(results, now) == [
        "[V4] 2.1_conformance :: Service returns service document (MUST)",
        "[V4] 2.1_conformance :: Twice #2",
    ]
    assert gained(results, now) == ["[V4] 2.1_conformance :: Service supports GET on single entities (MUST)"]
    assert lost(results, results) == []
    print("self-test: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
