#!/usr/bin/env python3
"""The cargo test arguments of partition K of N for the workspace in the current directory.

Splits the integration test targets (every workspace member's tests/*.rs, as `cargo metadata`
lists them) round robin by (package, name); partition 1 also takes every lib and bin target. A
workspace with an example, a bench or a `test = false` target is refused: `cargo test`'s default
treats those differently from an explicit target flag. The doc tests run as their own command
(`cargo test --doc` refuses other target flags). The union of the N partitions is every target
`cargo test --workspace` builds and runs, so a new tests/*.rs file joins a partition by itself.
  python3 test-part.py K N        # prints the arguments, one per line
  python3 test-part.py --selftest
"""
import json
import subprocess
import sys


def split(tests, k, n):
    """Partition k (1-based) of n: the libs and bins in 1, tests round robin."""
    own = [t for i, t in enumerate(sorted(tests)) if i % n == k - 1]
    args = ["--lib", "--bins"] if k == 1 else []
    for _, name in own:
        args += ["--test", name]
    return args


def selftest():
    tests = [("a", "t%d" % i) for i in range(7)] + [("b", "t0")]
    parts = [split(tests, k, 3) for k in (1, 2, 3)]
    named = [a for p in parts for a in p if a not in ("--test", "--lib", "--bins")]
    assert sorted(named) == sorted(n for _, n in tests), named
    assert parts[0][:2] == ["--lib", "--bins"] and "--lib" not in parts[1] + parts[2]
    print("test-part.py selftest ok")


def main():
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    k, n = int(sys.argv[1]), int(sys.argv[2])
    if not 1 <= k <= n:
        sys.exit(f"partition {k} of {n}?")
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"]))
    members = set(meta["workspace_members"])
    targets = [(p["name"], t) for p in meta["packages"] if p["id"] in members for t in p["targets"]]
    odd = [f"{p}:{t['name']}" for p, t in targets
           if not set(t["kind"]) <= {"lib", "bin", "test", "custom-build", "proc-macro"}
           or (t["kind"] != ["custom-build"] and not t["test"])]
    if odd:
        sys.exit(f"targets this split does not know: {odd}")
    tests = [(p, t["name"]) for p, t in targets if "test" in t["kind"]]
    if not tests:
        sys.exit("no test targets")
    print("\n".join(split(tests, k, n)))


main()
