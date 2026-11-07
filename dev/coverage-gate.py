#!/usr/bin/env python3
"""Coverage gate for the surfaces epic's new crates (surfaces-test-plan section 1, T-3491).

Every crate whose `[package.metadata.antares] role` is `ops`, `surface`, `protocol` or `ngsild` must cover
100 % of its lines and branches, minus what `ci/coverage-exclusions.toml` lists by crate, file and
function with the uncovered line and branch counts and a reason. The list cannot rot: an entry
whose function is gone, or whose counts no longer match what is uncovered there, is red too.

    dev/coverage-gate.py                     measure and check every such crate (by hand; no CI job since T-6147)
    dev/coverage-gate.py --shard K/N         only the K-th of N shards of them (one CI runner each, T-6035)
    dev/coverage-gate.py --check CRATE JSON  check one `cargo llvm-cov report --json` export
    dev/coverage-gate.py --self-test         one synthetic report per red condition, and a green one

A crate is measured by its own tests (`--all-features`) merged with the broker's tests under the
broker feature named by `[package.metadata.antares] feature`, when it names one. Existing crates
have no role and stay under T-3468's 95 % floor.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXCLUSIONS = ROOT / "ci/coverage-exclusions.toml"
ROLES = {"ops", "surface", "protocol", "ngsild"}


def mangled(path: str) -> str:
    """`a::b::c` as the length-prefixed run both Rust manglings (legacy and v0) contain."""
    return "".join(f"{len(seg)}{seg}" for seg in path.split("::"))


def merged(functions: list[dict]) -> dict[str, tuple[int, int]]:
    """`{"lines": (count, uncovered), "branches": (sides, uncovered)}` of functions across all their
    records: each test binary carries its own instance of a function (and of every generic or
    closure in it), so a line or a branch side is covered when any instance covered it. llvm's
    per-file summary does not merge that way (a line of a generic one instance missed counts as
    missed), so the totals are computed here, the same way as each exclusion's counts."""
    hit, missed, sides = set(), set(), {}
    for function in functions:
        for region in function.get("regions", []):
            start, end, count, kind = region[0], region[2], region[4], region[7]
            if kind != 0:  # code regions only; skipped and expansion regions carry no line of their own
                continue
            (hit if count > 0 else missed).update(range(start, end + 1))
        for b in function.get("branches", []):
            seen = sides.setdefault(tuple(b[:4]), [False, False])
            seen[0] |= b[4] > 0
            seen[1] |= b[5] > 0
    return {
        "lines": (len(hit | missed), len(missed - hit)),
        "branches": (2 * len(sides), sum((not t) + (not f) for t, f in sides.values())),
    }


def uncovered_in(functions: list[dict]) -> tuple[int, int]:
    """Uncovered lines and branch sides of one function across all its records."""
    m = merged(functions)
    return m["lines"][1], m["branches"][1]


def check(crate: str, crate_dir: str, report: dict, entries: list[dict]) -> list[str]:
    """Every reason this crate's report is red; empty when it is green."""
    data = report["data"][0]
    prefix = crate_dir.rstrip("/") + "/"
    files = [f for f in data.get("files", []) if f["filename"].startswith(prefix)]
    if not files:
        return [f"{crate}: the report has no file under {prefix}"]
    functions = [fn for fn in data.get("functions", []) if any(n.startswith(prefix) for n in fn.get("filenames", []))]
    totals = {k: [0, 0] for k in ("lines", "branches")}
    for f in files:
        per_file = merged([fn for fn in functions if fn["filenames"][0] == f["filename"]])
        for k, (count, uncovered) in per_file.items():
            totals[k][0] += count
            totals[k][1] += count - uncovered

    errors = []
    listed = {"lines": 0, "branches": 0}
    for e in entries:
        where = f"{crate} {e['file']} {e['function']}"
        listed["lines"] += e.get("lines", 0)
        listed["branches"] += e.get("branches", 0)
        records = [
            fn for fn in functions
            if mangled(e["function"]) in fn["name"] and fn["filenames"][0] == prefix + e["file"]
        ]
        if not records:
            errors.append(f"{where}: listed in {EXCLUSIONS.name} but no such function is measured")
            continue
        lines, branches = uncovered_in(records)
        if lines == 0 and branches == 0:
            errors.append(f"{where}: covered now; remove its entry")
        elif (lines, branches) != (e.get("lines", 0), e.get("branches", 0)):
            errors.append(
                f"{where}: lists {e.get('lines', 0)} lines, {e.get('branches', 0)} branches uncovered, "
                f"measured {lines} and {branches}"
            )
    for k, (count, covered) in totals.items():
        if covered < count - listed[k]:
            required = 100.0 if count == 0 else 100.0 * (count - listed[k]) / count
            measured = 100.0 if count == 0 else 100.0 * covered / count
            errors.append(f"{crate}: {k} {measured:.2f} % covered, {required:.2f} % required ({count - covered} uncovered, {listed[k]} listed)")
    return errors


def exclusions() -> list[dict]:
    entries = tomllib.loads(EXCLUSIONS.read_text()).get("exclusion", [])
    for e in entries:
        missing = {"crate", "file", "function", "reason"} - e.keys()
        if missing:
            sys.exit(f"{EXCLUSIONS}: an entry lacks {sorted(missing)}: {e}")
    return entries


def new_crates() -> list[dict]:
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    crates = []
    for p in meta["packages"]:
        antares = (p.get("metadata") or {}).get("antares") or {}
        if antares.get("role") in ROLES:
            crates.append({"name": p["name"], "dir": str(Path(p["manifest_path"]).parent), "feature": antares.get("feature")})
    return crates


def measure(crate: dict) -> dict:
    cov = ["cargo", "+nightly", "llvm-cov"]
    subprocess.run([*cov, "clean", "--workspace"], cwd=ROOT, check=True)
    subprocess.run([*cov, "--no-report", "--branch", "-p", crate["name"], "--all-features"], cwd=ROOT, check=True)
    if crate["feature"]:
        subprocess.run([*cov, "--no-report", "--branch", "-p", "antares-broker", "--features", crate["feature"], "--tests"],
                       cwd=ROOT, check=True)
    out = subprocess.run([*cov, "report", "--branch", "--json"], cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def shard(crates: list[dict], k: int, n: int) -> list[dict]:
    """The K-th (1-based) of N shards: crates with a broker feature (a broker test build each, the
    long ones) are dealt round-robin first, then the rest, so every shard gets its share of both."""
    order = sorted(crates, key=lambda c: (not c["feature"], c["name"]))
    return order[k - 1::n]


def main(k: int = 1, n: int = 1) -> int:
    entries = exclusions()
    crates = new_crates()
    if not crates:
        print("coverage-surfaces: no new crates (none has metadata.antares.role ops, surface, protocol or ngsild)")
        return 0
    mine_shard = shard(crates, k, n)
    errors = []
    for crate in mine_shard:
        mine = [e for e in entries if e["crate"] == crate["name"]]
        errors += check(crate["name"], crate["dir"], measure(crate), mine)
    if k == 1:  # once per run, not per shard
        known = {c["name"] for c in crates}
        errors += [f"{e['crate']}: listed in {EXCLUSIONS.name} but not a new crate" for e in entries if e["crate"] not in known]
    for e in errors:
        print(e)
    print(f"coverage-surfaces: shard {k}/{n}, {len(mine_shard)} of {len(crates)} crates "
          f"({', '.join(c['name'] for c in mine_shard)}), {'red' if errors else 'OK'}")
    return 1 if errors else 0


def self_test() -> int:
    d = "/w/crates/antares-x"
    parse = {
        "name": "_ZN9antares_x5parse5depth17h0123456789abcdefE",
        "filenames": [f"{d}/src/parse.rs"],
        # line 10 hit, line 11 never
        "regions": [[10, 1, 10, 9, 3, 0, 0, 0], [11, 5, 11, 20, 0, 0, 0, 0]],
        "branches": [[10, 4, 10, 8, 3, 0, 0, 0, 4]],
    }

    def report(functions, lines, branches, files_dir=d):
        return {"data": [{
            "files": [{"filename": f"{files_dir}/src/parse.rs", "summary": {
                "lines": {"count": lines[0], "covered": lines[1]},
                "branches": {"count": branches[0], "covered": branches[1]}}}],
            "functions": functions}]}

    entry = {"crate": "antares-x", "file": "src/parse.rs", "function": "antares_x::parse::depth", "lines": 1, "branches": 1, "reason": "r"}
    covered = dict(parse, regions=[[10, 1, 11, 20, 3, 0, 0, 0]], branches=[[10, 4, 10, 8, 3, 1, 0, 0, 4]])
    cases = [
        ("green: everything uncovered is listed", report([parse], (2, 1), (2, 1)), [entry], None),
        ("green: full coverage, nothing listed", report([covered], (2, 2), (2, 2)), [], None),
        ("below required", report([parse], (2, 1), (2, 1)), [], "required"),
        ("a listed entry is covered now", report([covered], (2, 2), (2, 2)), [entry], "covered now"),
        ("an entry names a missing function", report([parse], (2, 1), (2, 1)), [dict(entry, function="antares_x::parse::gone")], "no such function"),
        ("an entry's count is stale", report([parse], (2, 1), (2, 1)), [dict(entry, lines=2)], "measured 1"),
        ("green: an instance in another test binary covers what this one misses",
         report([parse, dict(covered, name="_RNvNtCs1234_9antares_x5parse5depth")], (2, 2), (2, 2)), [], None),
        ("an entry is checked across instances, not per instance",
         report([parse, dict(parse, name="_RNvNtCs1234_9antares_x5parse5depth", regions=[[11, 5, 11, 20, 2, 0, 0, 0]])],
                (2, 2), (2, 1)), [dict(entry, lines=0)], None),
        ("no file of the crate in the report", report([parse], (2, 1), (2, 1), files_dir="/w/crates/other"), [], "no file under"),
    ]
    failed = 0
    many = [{"name": f"c{i:02}", "dir": "", "feature": "f" if i % 3 == 0 else None} for i in range(31)]
    for n in (1, 2, 3, 4):
        dealt = [c["name"] for k in range(1, n + 1) for c in shard(many, k, n)]
        sizes = [len(shard(many, k, n)) for k in range(1, n + 1)]
        heavy = [sum(1 for c in shard(many, k, n) if c["feature"]) for k in range(1, n + 1)]
        ok = sorted(dealt) == sorted(c["name"] for c in many) and max(sizes) - min(sizes) <= 1 and max(heavy) - min(heavy) <= 1
        print(f"{'ok  ' if ok else 'FAIL'} {n} shards measure every crate once, evenly: sizes {sizes}, with a feature {heavy}")
        failed += not ok
    for name, rep, entries, red in cases:
        errors = check("antares-x", d, rep, entries)
        ok = not errors if red is None else any(red in e for e in errors)
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {errors or 'green'}")
        failed += not ok
    return 1 if failed else 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        sys.exit(self_test())
    if len(sys.argv) == 4 and sys.argv[1] == "--check":
        crate = next((c for c in new_crates() if c["name"] == sys.argv[2]), None)
        if crate is None:
            sys.exit(f"{sys.argv[2]} is not a new crate")
        errs = check(crate["name"], crate["dir"], json.loads(Path(sys.argv[3]).read_text()),
                     [e for e in exclusions() if e["crate"] == crate["name"]])
        print("\n".join(errs) or "OK")
        sys.exit(1 if errs else 0)
    if len(sys.argv) == 3 and sys.argv[1] == "--shard":
        k, _, n = sys.argv[2].partition("/")
        if not (k.isdigit() and n.isdigit() and 1 <= int(k) <= int(n)):
            sys.exit(f"--shard wants K/N with 1 <= K <= N, not {sys.argv[2]}")
        sys.exit(main(int(k), int(n)))
    if sys.argv[1:]:
        sys.exit(__doc__)
    sys.exit(main())
