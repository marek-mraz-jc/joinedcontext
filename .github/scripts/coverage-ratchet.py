#!/usr/bin/env python3
"""Per-crate coverage table and ratchet (T-7529): no crate's line or function coverage may drop.

`cargo llvm-cov report --json` exports (one per workspace or shard, plain or .gz) are folded per
crate: each source file belongs to the package of the nearest Cargo.toml above it, and files
under a crate's tests/, benches/ or examples/ (test fixtures and helpers, not the code the
floors guard) are left out. A crate's percent is covered/count of its files, truncated to two
decimals.

Lines and functions are merged across a function's instances: a crate compiled twice (two crate
hashes, as a dev-dependency of two test binaries) or a generic monomorphised in several binaries
gives one record per instance, and llvm's own file summary counts a line missed when any instance
missed it. Here a function is covered when any instance ran, and a line when any instance ran
code on it (the region wrapping the line or one starting on it, as llvm reads a line). An export
made with --summary-only carries no records and is read from llvm's summary as it is. `.github/coverage-ratchet.toml` holds one
floor per crate (`"name" = { lines = 96.61, functions = 95.02 }`).

    coverage-ratchet.py table [--mutants OUTCOMES.json ...] [--md FILE] [--json FILE] EXPORT.json ...
        the per-crate table (lines, functions, missed mutants when outcomes are given)
    coverage-ratchet.py check [--ratchet FILE] EXPORT.json ...
        red when a measured crate is below its floor or a floor names a crate no Cargo.toml in the
        repository declares any more (a deleted crate drops its line in the same commit); a measured
        crate without a floor is a warning (the nightly's artifact carries the updated file to
        commit); crates this export did not measure are not judged (shards)
    coverage-ratchet.py floors [--ratchet FILE]
        only the deleted-crate rule, without an export: ci.yml runs it on every branch, so a branch
        that deletes or renames a crate drops or renames its floor in the same commit
    coverage-ratchet.py update [--ratchet FILE] EXPORT.json ...
        raises each measured crate's floor to what it measured and adds new crates; a floor never
        goes down here: lowering one (a well-covered file deleted) is a hand edit a reviewer reads
    coverage-ratchet.py --selftest
        one synthetic repository per outcome, red unless each is judged as expected
"""

import gzip
import json
import math
import pathlib
import sys
import tempfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
RATCHET = ROOT / ".github/coverage-ratchet.toml"
METRICS = ("lines", "functions")
NOT_MEASURED = {"tests", "benches", "examples"}
# A crate is red only when it measures more than this many points below its floor: an async or
# timing-dependent branch can move a crate by a line between two runs of the same commit.
TOLERANCE = 0.1


def package_of(path, root, cache):
    """The `[package] name` of the nearest Cargo.toml above `path` inside `root`, else None."""
    d = path.parent
    while True:
        if d in cache:
            return cache[d]
        manifest = d / "Cargo.toml"
        if manifest.is_file():
            name = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
            if name:
                cache[d] = name
                return name
        if d == root or d.parent == d:
            cache[d] = None
            return None
        d = d.parent


def declared_crates(root):
    """Every package name a Cargo.toml under `root` declares (target/ and .git skipped)."""
    names = set()
    for manifest in root.rglob("Cargo.toml"):
        if {"target", ".git", "node_modules"} & set(manifest.relative_to(root).parts):
            continue
        name = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
        if name:
            names.add(name)
    return names


def read_export(path):
    raw = pathlib.Path(path).read_bytes()
    return json.loads(gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw)


def line_counts(regions):
    """{line: count} of one function record: per line the larger of the innermost code region
    wrapping into it from an earlier line and the regions starting on it."""
    code = [r for r in regions if r[7] == 0]  # code regions; skipped and expansion ones carry no line
    counts = {}
    for line in {n for r in code for n in range(r[0], r[2] + 1)}:
        starting = [r[4] for r in code if r[0] == line]
        wrapping = [r for r in code if r[0] < line <= r[2]]
        inner = max(wrapping, key=lambda r: (r[0], r[1]))[4] if wrapping else 0
        counts[line] = max([inner, *starting])
    return counts


def merged_files(data):
    """{file: summary} from a full export's function records, instances merged (module doc)."""
    hit, seen, funcs = {}, {}, {}
    for fn in data.get("functions", []):
        code = [r for r in fn["regions"] if r[7] == 0]
        if not code:
            continue
        name = fn["filenames"][0]
        key = (name, code[0][0], code[0][1])
        funcs[key] = funcs.get(key, False) or fn["count"] > 0
        for line, count in line_counts(fn["regions"]).items():
            seen.setdefault(name, set()).add(line)
            if count > 0:
                hit.setdefault(name, set()).add(line)
    out = {}
    for name, lines in seen.items():
        covered = [v for (f, _, _), v in funcs.items() if f == name]
        out[name] = {"lines": {"count": len(lines), "covered": len(hit.get(name, ()))},
                     "functions": {"count": len(covered), "covered": sum(covered)}}
    return out


def per_file(exports):
    """{absolute file: summary}; a file two exports measured keeps the better-covered record."""
    files = {}
    for export in exports:
        for data in read_export(export)["data"]:
            merged = merged_files(data) if data.get("functions") else None
            for f in data["files"]:
                s = merged.get(f["filename"]) if merged is not None else f["summary"]
                if s is None:
                    continue
                old = files.get(f["filename"])
                if old is None or s["lines"]["covered"] > old["lines"]["covered"]:
                    files[f["filename"]] = s
    return files


def per_crate(exports, root):
    """{crate: {"lines": (count, covered), "functions": (count, covered)}} of files inside `root`."""
    crates, cache = {}, {}
    for name, s in per_file(exports).items():
        path = pathlib.Path(name)
        if not path.is_relative_to(root):
            continue
        crate = package_of(path, root, cache)
        if crate is None or NOT_MEASURED & set(path.relative_to(root).parts):
            continue
        acc = crates.setdefault(crate, {m: [0, 0] for m in METRICS})
        for m in METRICS:
            acc[m][0] += s[m]["count"]
            acc[m][1] += s[m]["covered"]
    return {c: {m: tuple(v[m]) for m in METRICS} for c, v in crates.items()}


def percent(count, covered):
    """covered/count in percent, truncated (never rounded up) to two decimals; 100 for no code."""
    return 100.0 if count == 0 else math.floor(covered * 10000 / count) / 100


def missed_mutants(outcome_files):
    """{package: missed mutant count} from cargo-mutants outcomes.json files."""
    missed = {}
    for path in outcome_files:
        for o in json.loads(pathlib.Path(path).read_text()).get("outcomes", []):
            scenario = o.get("scenario")
            if not isinstance(scenario, dict) or "Mutant" not in scenario:
                continue
            package = scenario["Mutant"].get("package", "?")
            missed.setdefault(package, 0)
            if o.get("summary") == "MissedMutant":
                missed[package] += 1
    return missed


def table(crates, missed):
    rows = ["| crate | lines | functions | missed mutants |", "|---|---:|---:|---:|"]
    for c in sorted(crates):
        cells = [f"{percent(*crates[c][m]):.2f} % ({crates[c][m][1]}/{crates[c][m][0]})" for m in METRICS]
        rows.append(f"| {c} | {cells[0]} | {cells[1]} | {missed.get(c, 'not run')} |")
    return "\n".join(rows) + "\n"


def load_ratchet(path):
    return tomllib.loads(path.read_text()).get("crates", {}) if path.is_file() else {}


def write_ratchet(path, floors):
    lines = [
        "# Per-crate coverage floors (T-7529): the nightly coverage workflow fails when a crate measures below",
        "# its line. `.github/scripts/coverage-ratchet.py update EXPORT.json` raises them; a branch that raises a",
        "# crate commits the new line with it. Lowering a floor is a hand edit with its reason in the commit.",
        "[crates]",
    ]
    for c in sorted(floors):
        f = floors[c]
        lines.append(f'"{c}" = {{ lines = {f["lines"]:.2f}, functions = {f["functions"]:.2f} }}')
    path.write_text("\n".join(lines) + "\n")


def unfloored(crates, floors):
    return [f"{c}: measured but has no floor; commit the line `coverage-ratchet.py update` writes"
            for c in sorted(crates) if c not in floors]


def stale(floors, declared):
    return [f"{c}: no Cargo.toml declares this crate any more; remove its floor"
            for c in sorted(floors) if c not in declared]


def problems(crates, floors, declared):
    found = stale(floors, declared)
    for c in sorted(crates):
        if c not in floors:
            continue
        for m in METRICS:
            got, floor = percent(*crates[c][m]), floors[c][m]
            if got < floor - TOLERANCE:
                found.append(f"{c}: {m} {got:.2f} % is below its floor {floor:.2f} %")
    return found


def raised(crates, floors):
    out = {c: dict(f) for c, f in floors.items()}
    for c, v in crates.items():
        old = out.get(c, {m: 0.0 for m in METRICS})
        out[c] = {m: max(old[m], percent(*v[m])) for m in METRICS}
    return out


def option(args, name, many=False):
    """Pops `name VALUE` pairs out of args; a list when `many`, else the last value or None."""
    values = []
    while name in args:
        i = args.index(name)
        values.append(args[i + 1])
        del args[i:i + 2]
    return values if many else (values[-1] if values else None)


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if not argv or argv[0] not in ("table", "check", "update", "floors"):
        print(__doc__, file=sys.stderr)
        return 2
    command, args = argv[0], list(argv[1:])
    ratchet = pathlib.Path(option(args, "--ratchet") or RATCHET)
    root = pathlib.Path(option(args, "--root") or ROOT).resolve()
    mutants, md, js = option(args, "--mutants", many=True), option(args, "--md"), option(args, "--json")
    if command == "floors":
        found = stale(load_ratchet(ratchet), declared_crates(root))
        for line in found:
            print(f"coverage-ratchet: {line}")
        return 1 if found else 0
    if not args:
        print("coverage-ratchet: no llvm-cov export given", file=sys.stderr)
        return 2
    crates = per_crate(args, root)
    if command == "table":
        text = table(crates, missed_mutants(mutants))
        print(text, end="")
        if md:
            pathlib.Path(md).write_text(text)
        if js:
            report = {c: {m: {"count": v[m][0], "covered": v[m][1], "percent": percent(*v[m])} for m in METRICS}
                      for c, v in crates.items()}
            for c, n in missed_mutants(mutants).items():
                report.setdefault(c, {})["missed_mutants"] = n
            pathlib.Path(js).write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        return 0
    floors = load_ratchet(ratchet)
    if command == "update":
        write_ratchet(ratchet, raised(crates, floors))
        print(f"coverage-ratchet: {len(crates)} crates measured, floors written to {ratchet}")
        return 0
    found = problems(crates, floors, declared_crates(root))
    for line in unfloored(crates, floors):
        print(f"::warning::coverage-ratchet: {line}")
    for line in found:
        print(f"coverage-ratchet: {line}")
    print(f"coverage-ratchet: {len(crates)} crates judged, {len(found)} red")
    return 1 if found else 0


def selftest():
    def export(path, files):
        data = [{"files": [{"filename": str(f), "summary": {
            "lines": {"count": lc, "covered": lv}, "functions": {"count": fc, "covered": fv}}}
            for f, (lc, lv, fc, fv) in files.items()]}]
        path.write_text(json.dumps({"data": data}))

    failed, cases = [], 0
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name in ("alpha", "beta"):
            (root / name / "src").mkdir(parents=True)
            (root / name / "Cargo.toml").write_text(f'[package]\nname = "{name}"\n')
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["alpha", "beta"]\n')
        a, b = root / "alpha/src/lib.rs", root / "beta/src/lib.rs"
        first, second = root / "one.json", root / "two.json"
        export(first, {a: (200, 199, 10, 10), "/home/x/.cargo/registry/dep.rs": (5, 0, 1, 0)})
        export(second, {b: (3, 2, 3, 3), a: (200, 150, 10, 9), root / "beta/tests/it.rs": (9, 0, 2, 0)})
        ratchet = root / "ratchet.toml"

        def judge(name, got, want):
            nonlocal cases
            cases += 1
            if got != want:
                failed.append(f"{name}: got {got!r}, want {want!r}")

        crates = per_crate([first, second], root)
        judge("two exports fold per crate, better record wins, foreign and tests/ files dropped", crates,
              {"alpha": {"lines": (200, 199), "functions": (10, 10)}, "beta": {"lines": (3, 2), "functions": (3, 3)}})
        judge("percent truncates 199/200 to 99.50", percent(200, 199), 99.5)
        judge("percent never rounds 2/3 up", percent(3, 2), 66.66)
        judge("no code counts as covered", percent(0, 0), 100.0)
        declared = declared_crates(root)
        judge("at the floor is green", problems(crates, {"alpha": {"lines": 99.5, "functions": 100.0},
                                                         "beta": {"lines": 66.66, "functions": 100.0}}, declared), [])
        judge("a drop within the tolerance is green",
              problems(crates, {"alpha": {"lines": 99.6, "functions": 100.0}}, declared), [])
        judge("a drop is red", problems(crates, {"alpha": {"lines": 99.61, "functions": 100.0},
                                                 "beta": {"lines": 0.0, "functions": 0.0}}, declared),
              ["alpha: lines 99.50 % is below its floor 99.61 %"])
        judge("a new crate without a floor is a warning, not red",
              (problems(crates, {"alpha": {"lines": 0.0, "functions": 0.0}}, declared),
               unfloored(crates, {"alpha": {"lines": 0.0, "functions": 0.0}})),
              ([], ["beta: measured but has no floor; commit the line `coverage-ratchet.py update` writes"]))
        judge("a deleted crate's floor is red", problems({}, {"gamma": {"lines": 50.0, "functions": 50.0}}, declared),
              ["gamma: no Cargo.toml declares this crate any more; remove its floor"])
        judge("an unmeasured (other shard) crate is not judged",
              problems({}, {"alpha": {"lines": 100.0, "functions": 100.0}}, declared), [])
        judge("update raises and never lowers", raised(crates, {"alpha": {"lines": 99.9, "functions": 1.0}}),
              {"alpha": {"lines": 99.9, "functions": 100.0}, "beta": {"lines": 66.66, "functions": 100.0}})
        write_ratchet(ratchet, raised(crates, {}))
        judge("the written ratchet reads back", load_ratchet(ratchet),
              {"alpha": {"lines": 99.5, "functions": 100.0}, "beta": {"lines": 66.66, "functions": 100.0}})
        judge("check exits 0 on its own update", main(["check", "--root", tmp, "--ratchet", str(ratchet),
                                                        str(first), str(second)]), 0)
        judge("floors is green while every floor's crate exists",
              main(["floors", "--root", tmp, "--ratchet", str(ratchet)]), 0)
        write_ratchet(root / "stale.toml", {"gone": {"lines": 1.0, "functions": 1.0}})
        judge("floors is red on a deleted crate's floor",
              main(["floors", "--root", tmp, "--ratchet", str(root / "stale.toml")]), 1)
        export(second, {b: (3, 1, 3, 3)})
        judge("check exits 1 on a forced drop", main(["check", "--root", tmp, "--ratchet", str(ratchet),
                                                       str(first), str(second)]), 1)
        # Two instances of one function (lines 10-14): one never ran, the other ran all but the
        # `else` block from line 12 column 5 to line 13 (line 12 still ran code before it, so only
        # line 13 is missed); a second function on line 20 never ran in either.
        body = [[10, 1, 14, 2, 1, 0, 0, 0], [12, 5, 13, 6, 0, 0, 0, 0]]
        full = root / "full.json.gz"
        full.write_bytes(gzip.compress(json.dumps({"data": [{"files": [
            {"filename": str(a), "summary": {"lines": {"count": 6, "covered": 0},
                                             "functions": {"count": 3, "covered": 0}}}],
            "functions": [
                {"name": "inst_a", "count": 0, "filenames": [str(a)],
                 "regions": [[10, 1, 14, 2, 0, 0, 0, 0], [12, 5, 13, 6, 0, 0, 0, 0]]},
                {"name": "inst_b", "count": 1, "filenames": [str(a)], "regions": body},
                {"name": "other", "count": 0, "filenames": [str(a)], "regions": [[20, 1, 20, 9, 0, 0, 0, 0]]},
            ]}]}).encode()))
        judge("instances merge: a line any instance ran is covered, a function any instance ran too",
              per_crate([full], root), {"alpha": {"lines": (6, 4), "functions": (2, 1)}})
        judge("a region starting on a line counts for it beside the wrapping one",
              line_counts([[1, 1, 3, 1, 0, 0, 0, 0], [2, 9, 2, 20, 4, 0, 0, 0], [3, 1, 3, 1, 0, 0, 0, 2]]),
              {1: 0, 2: 4, 3: 0})
        outcomes = root / "outcomes.json"
        outcomes.write_text(json.dumps({"outcomes": [
            {"scenario": "Baseline", "summary": "Success"},
            {"scenario": {"Mutant": {"package": "alpha"}}, "summary": "MissedMutant"},
            {"scenario": {"Mutant": {"package": "alpha"}}, "summary": "CaughtMutant"},
            {"scenario": {"Mutant": {"package": "beta"}}, "summary": "Unviable"}]}))
        judge("missed mutants count per package", missed_mutants([outcomes]), {"alpha": 1, "beta": 0})
    if failed:
        print("coverage-ratchet selftest FAILED:\n  " + "\n  ".join(failed))
        return 1
    print(f"coverage-ratchet selftest: {cases} cases judged as expected")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
