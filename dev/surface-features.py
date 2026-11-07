#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""The surface features of antares-broker, and the binary size each one costs
(ADR-0027 sections 4 and 6, T-3493).

A surface feature is one that pulls in a crate of role `surface`
(`dep:<crate>`, module rule M1), plus `full` once the broker has it; a new
surface joins by being declared, without a YAML edit.

  dev/surface-features.py list         comma-separated, for cargo hack
  dev/surface-features.py sizes [k/n] build vanilla and each surface feature
                                       release, report, gate against the budget;
                                       with k/n only every n-th feature from the
                                       k-th, beside its own vanilla build (T-4680)
  dev/surface-features.py --self-test  the gate is red on a synthetic overrun

Budget: the "binary size" row of ADR-0027 section 6, read from the ADR so
the gate cannot drift from it (`full` and each feature alone <= vanilla + 15 MiB, T-5246),
measured as the size of the stripped release binary (the release profile strips
symbols). The builds run in lanes of their own target directory, side by side
when CARGO_BUILD_JOBS allows (T-4032): one lane below 8 jobs, else one per 4 jobs,
at most 3, the jobs split between them. A lane builds its variants in a fixed
order, so the size of a build does not depend on how many lanes there are. The report is a Markdown table on stdout and, in GitHub Actions,
in the step summary.
"""

import json
import os
import pathlib
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

MIB = 1024 * 1024
BROKER = "antares-broker"
ADR = pathlib.Path(__file__).resolve().parent.parent / "docs/adr/ADR-0027-surfaces-architecture.md"


def budgets(text):
    """`(named, default)` in bytes from the ADR's `| binary size |` row: a
    feature in backticks followed by `≤ vanilla + N MiB`, and `each feature
    alone ≤ vanilla + N MiB` for the rest."""
    row = next((line for line in text.splitlines() if line.startswith("| binary size |")), None)
    if row is None:
        sys.exit("ADR-0027 has no `| binary size |` row: no budget to gate against")
    rule = row.split("|")[2]
    named = {f: float(n) * MIB for f, n in re.findall(r"`([a-z0-9-]+)`[^`≤]*≤ vanilla \+ ([\d.]+) MiB", rule)}
    default = re.search(r"each feature alone ≤ vanilla \+ ([\d.]+) MiB", rule)
    if default is None or "full" not in named:
        sys.exit(f"ADR-0027's binary size row does not read as a budget: {rule.strip()}")
    return named, float(default.group(1)) * MIB


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def surface_features(meta):
    role = {
        p["name"]: ((p.get("metadata") or {}).get("antares") or {}).get("role")
        for p in meta["packages"]
    }
    broker = next(p for p in meta["packages"] if p["name"] == BROKER)
    found = sorted(
        name
        for name, enables in broker["features"].items()
        if any(e.startswith("dep:") and role.get(e[4:]) == "surface" for e in enables)
    )
    if "full" in broker["features"]:
        found.append("full")
    if not found:
        sys.exit(f"{BROKER} declares no surface feature: the gate would pass vacuously")
    return found


def verdicts(vanilla, sizes, budget):
    """One row per feature: (feature, size, over vanilla, budget, within)."""
    named, default = budget
    rows = []
    for feature, size in sizes.items():
        budget = named.get(feature, default)
        rows.append((feature, size, size - vanilla, budget, size - vanilla <= budget))
    return rows


def report(vanilla, rows):
    lines = [
        "## Binary size per surface feature (ADR-0027 section 6)",
        "",
        "| build | bytes | over vanilla | budget | |",
        "|---|---:|---:|---:|---|",
        f"| vanilla | {vanilla} | | | |",
    ]
    for feature, size, over, budget, ok in rows:
        lines.append(
            f"| `{feature}` | {size} | {over / MIB:+.2f} MiB | {budget / MIB:g} MiB | {'ok' if ok else 'OVER'} |"
        )
    return "\n".join(lines) + "\n"


def build(target_dir, features, jobs):
    cmd = ["cargo", "build", "--release", "--locked", "-p", BROKER, "--jobs", str(jobs)]
    if features:
        cmd += ["--features", features]
    subprocess.run(cmd, check=True, env={**os.environ, "CARGO_TARGET_DIR": str(target_dir)})
    return pathlib.Path(target_dir, "release", "antares").stat().st_size


def lanes(jobs, builds):
    """How many builds run side by side: one below 8 jobs, so a small worker does not thrash,
    else one per 4 jobs, at most 3 and never more than there are builds."""
    return 1 if jobs < 8 else max(1, min(3, jobs // 4, builds))


def share(features, part):
    """The features of partition `k/n` (1-based, cargo-hack's notation); all of them for None."""
    if part is None:
        return features
    m = re.fullmatch(r"([1-9][0-9]*)/([1-9][0-9]*)", part)
    if not m or int(m[1]) > int(m[2]):
        sys.exit(f"partition {part!r}: want k/n with 1 <= k <= n")
    k, n = int(m[1]), int(m[2])
    return features[k - 1::n]


def sizes(part=None):
    meta = metadata()
    target = pathlib.Path(meta["target_directory"])
    # Every partition measures vanilla itself: a size is only comparable within one runner.
    features = [None] + share(surface_features(meta), part)
    jobs = int(os.environ.get("CARGO_BUILD_JOBS") or os.cpu_count() or 1)
    n = lanes(jobs, len(features))
    # Lane 0 is the target directory itself, the others a directory of their own beside it: two
    # release builds never share one, because cargo locks a profile directory per build.
    dirs = [target] + [target / f"sizes-{k}" for k in range(1, n)]

    def lane(k):
        return {f: build(dirs[k], f, max(1, jobs // n)) for f in features[k::n]}

    with ThreadPoolExecutor(n) as pool:
        measured = {f: size for part in pool.map(lane, range(n)) for f, size in part.items()}
    vanilla = measured[None]
    budget = budgets(ADR.read_text())
    rows = verdicts(vanilla, {f: measured[f] for f in features[1:]}, budget)
    text = report(vanilla, rows)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(text)
    over = [r[0] for r in rows if not r[4]]
    if over:
        sys.exit(f"over the ADR-0027 size budget: {', '.join(over)}")


def self_test():
    vanilla = 30 * MIB
    adr = budgets(ADR.read_text())
    if adr != ({"full": 15 * MIB}, 15 * MIB):
        sys.exit(f"self-test: the ADR's budget reads as {adr}")
    rows = verdicts(vanilla, {"ogc": vanilla + 15 * MIB, "full": vanilla + 15 * MIB + 1}, adr)
    if {r[0]: r[4] for r in rows} != {"ogc": True, "full": False}:
        sys.exit(f"self-test: the ADR's 15 MiB budget does not hold at +15 MiB or fails at +1 byte: {rows}")
    rule = "| binary size | `full` ≤ vanilla + 6 MiB; each feature alone ≤ vanilla + 2 MiB, except `mcp` ≤ vanilla + 4.5 MiB | x |"
    budget = budgets(rule)
    if budget != ({"full": 6 * MIB, "mcp": 4.5 * MIB}, 2 * MIB):
        sys.exit(f"self-test: a row with exceptions reads as {budget}")
    rows = verdicts(vanilla, {"views": vanilla + 2 * MIB, "ws": vanilla + 2 * MIB + 1,
                              "mcp": vanilla + 4.5 * MIB, "full": vanilla + 6 * MIB + 1}, budget)
    got = {r[0]: r[4] for r in rows}
    if got != {"views": True, "ws": False, "mcp": True, "full": False}:
        sys.exit(f"self-test: verdicts {got}")
    if "4.5 MiB" not in report(vanilla, rows):
        sys.exit("self-test: a fractional budget is not reported as it is")
    if "OVER" not in report(vanilla, rows):
        sys.exit("self-test: an overrun is not marked in the report")
    meta = {"packages": [
        {"name": BROKER, "features": {
            "default": ["mqtt"], "mqtt": ["antares-api/mqtt"], "views": ["dep:antares-ops", "dep:antares-views"],
            "mcp": ["views", "dep:antares-mcp"], "plugin-example": ["dep:antares-plugin-example"],
        }},
        {"name": "antares-views", "metadata": {"antares": {"role": "surface"}}},
        {"name": "antares-mcp", "metadata": {"antares": {"role": "surface"}}},
        {"name": "antares-ops", "metadata": {"antares": {"role": "ops"}}},
        {"name": "antares-plugin-example", "metadata": {"antares": {"role": "addon"}}},
    ]}
    if surface_features(meta) != ["mcp", "views"]:
        sys.exit(f"self-test: surface features {surface_features(meta)}")
    meta["packages"][0]["features"]["full"] = ["mcp"]
    if surface_features(meta) != ["mcp", "views", "full"]:
        sys.exit("self-test: full is not picked up")
    if [lanes(j, 9) for j in (1, 3, 7, 8, 12, 16, 64)] != [1, 1, 1, 2, 3, 3, 3] or lanes(12, 2) != 2:
        sys.exit("self-test: the lanes do not follow CARGO_BUILD_JOBS")
    names = ["ical", "mcp", "views", "ws", "full"]
    parts = [share(names, f"{k}/3") for k in (1, 2, 3)]
    if parts != [["ical", "ws"], ["mcp", "full"], ["views"]] or share(names, None) != names:
        sys.exit(f"self-test: partitions {parts}")
    if sorted(sum(parts, [])) != sorted(names):
        sys.exit("self-test: the partitions do not cover every feature once")
    print("surface-features: self-test passed")


if __name__ == "__main__":
    mode = sys.argv[1:] or ["list"]
    if mode == ["--self-test"]:
        self_test()
    elif mode[0] == "sizes" and len(mode) <= 2:
        sizes(mode[1] if len(mode) == 2 else None)
    elif mode == ["list"]:
        print(",".join(surface_features(metadata())))
    else:
        sys.exit(__doc__)
