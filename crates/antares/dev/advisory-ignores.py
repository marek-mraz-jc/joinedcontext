#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""The `cargo audit` ignore flags, read from `deny.toml` (surfaces-threat-model
11.1): the advisories cargo-deny is told to ignore are the only ones
cargo-audit ignores, so the two tools cannot drift.

  dev/advisory-ignores.py [deny.toml]   one `--ignore <id>` pair per entry
  dev/advisory-ignores.py --self-test   a deny.toml with two ignores gives two

Entries are `{ id = "...", reason = "..." }` tables or bare id strings, as
cargo-deny takes them. An entry without an id is an error, not a skip: a
silently dropped ignore would turn into a red audit nobody can explain.
"""

import pathlib
import sys
import tempfile
import tomllib


def ignores(path):
    with open(path, "rb") as f:
        listed = tomllib.load(f).get("advisories", {}).get("ignore", [])
    out = []
    for entry in listed:
        rid = entry.get("id") if isinstance(entry, dict) else entry
        if not isinstance(rid, str) or not rid:
            sys.exit(f"{path}: an [advisories] ignore entry has no id: {entry!r}")
        out += ["--ignore", rid]
    return out


def self_test():
    toml = """[advisories]
ignore = [
    { id = "RUSTSEC-2024-0001", reason = "a table" },
    "RUSTSEC-2024-0002",
]
[bans]
deny = [{ name = "openssl" }]
"""
    with tempfile.TemporaryDirectory() as d:
        p = pathlib.Path(d, "deny.toml")
        p.write_text(toml)
        got = ignores(p)
        want = ["--ignore", "RUSTSEC-2024-0001", "--ignore", "RUSTSEC-2024-0002"]
        if got != want:
            sys.exit(f"self-test: {got} != {want}")
        p.write_text("[advisories]\n")
        if ignores(p) != []:
            sys.exit("self-test: no ignore list must give no flags")
    print("advisory-ignores: self-test passed")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    else:
        print(" ".join(ignores(sys.argv[1] if len(sys.argv) > 1 else "deny.toml")))
