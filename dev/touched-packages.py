#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Does a change reach a package? The skip rule of fast's "Each feature alone" shards (T-6035).

    dev/touched-packages.py BASE TARGET   print `true` (run the shard) or `false` (skip it)
    dev/touched-packages.py --self-test   one case per rule

TARGET is a workspace package, `workspace` (any package) or `binary-size` (antares-broker's
release builds). The change is `git diff --name-only BASE HEAD`. The shard runs when:

- BASE is empty or git cannot diff it (a dispatch without a base, a lost commit): nothing to
  compare with, so everything runs;
- a changed file lies outside every package directory (Cargo.lock, the root manifest, a
  workflow, ci/, docs/, contexts/, ngsi-ld-test-suite/): crates read such files at build and
  test time (include_str!, CARGO_MANIFEST_DIR paths), so the rule never guesses which;
- a changed file lies in TARGET or in a package TARGET reaches through its path dependencies
  (normal, build, dev and optional ones, followed transitively).

Otherwise it skips, and says which packages the change touched. Unlike dev/preflight.sh, which
checks the touched crates themselves, a shard here tests its package against every crate below
it, so the rule follows the dependency edges.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ALIASES = {"binary-size": "antares-broker"}


def packages() -> dict[str, dict]:
    """name -> {"dir": path relative to the root, "deps": workspace packages it names by path}"""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    return {p["name"]: {"dir": os.path.relpath(os.path.dirname(p["manifest_path"]), ROOT),
                        "deps": {d["name"] for d in p["dependencies"] if d.get("path")}}
            for p in meta["packages"]}


def reached(pkgs: dict[str, dict], target: str) -> set[str]:
    seen, todo = set(), [target]
    while todo:
        name = todo.pop()
        if name in seen or name not in pkgs:
            continue
        seen.add(name)
        todo += pkgs[name]["deps"]
    return seen


def decide(changed: list[str], pkgs: dict[str, dict], target: str) -> tuple[bool, str]:
    target = ALIASES.get(target, target)
    if target != "workspace" and target not in pkgs:
        return True, f"{target} is no workspace package; running"
    by_dir = sorted(((p["dir"], n) for n, p in pkgs.items() if p["dir"] != "."), key=lambda d: -len(d[0]))
    owners = set()
    for f in changed:
        owner = next((n for d, n in by_dir if f.startswith(d + "/")), None)
        if owner is None:
            return True, f"{f} lies outside every package"
        owners.add(owner)
    want = set(pkgs) if target == "workspace" else reached(pkgs, target)
    hit = owners & want
    if hit:
        return True, f"the change touches {', '.join(sorted(hit))}"
    return False, f"the change touches only {', '.join(sorted(owners)) or 'nothing'}, which {target} does not reach"


def changed_files(base: str) -> list[str] | None:
    r = subprocess.run(["git", "diff", "--name-only", base, "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.split() if r.returncode == 0 else None


def self_test() -> int:
    pkgs = {
        "root": {"dir": ".", "deps": set()},
        "app": {"dir": "crates/app", "deps": {"mid"}},
        "mid": {"dir": "crates/mid", "deps": {"leaf"}},
        "leaf": {"dir": "crates/leaf", "deps": set()},
        "other": {"dir": "crates/other", "deps": set()},
        "app-extra": {"dir": "crates/app-extra", "deps": set()},
        "antares-broker": {"dir": "crates/antares-broker", "deps": set()},
    }
    cases = [
        ("a touched package runs", ["crates/app/src/lib.rs"], "app", True),
        ("a crate two edges below runs", ["crates/leaf/src/lib.rs"], "app", True),
        ("an unreached crate skips", ["crates/other/src/lib.rs"], "app", False),
        ("a sibling directory with the same prefix skips", ["crates/app-extra/src/lib.rs"], "app", False),
        ("the lock file runs", ["Cargo.lock"], "app", True),
        ("a file outside every package runs", ["docs/src/configuration.md"], "leaf", True),
        ("a workflow runs", [".github/workflows/workspace.yml"], "leaf", True),
        ("workspace runs on any package", ["crates/other/src/lib.rs"], "workspace", True),
        ("binary-size is antares-broker", ["crates/antares-broker/src/main.rs"], "binary-size", True),
        ("binary-size skips what the broker does not reach", ["crates/other/src/lib.rs"], "binary-size", False),
        ("an unknown target runs", ["crates/other/src/lib.rs"], "nope", True),
        ("an empty change skips", [], "app", False),
    ]
    failed = 0
    for name, changed, target, want in cases:
        got, why = decide(changed, pkgs, target)
        ok = got == want
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {got} ({why})")
        failed += not ok
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 2:
        sys.exit(__doc__)
    base, target = argv
    changed = changed_files(base) if base else None
    if changed is None:
        run, why = True, f"no base to compare with ({base or 'none given'}); running"
    else:
        run, why = decide(changed, packages(), target)
    print(f"{target}: {'run' if run else 'skip'}: {why}", file=sys.stderr)
    print("true" if run else "false")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
