#!/usr/bin/env python3
"""Fails when a registry or git package of LOCK is not in ROOT at the same version and source:
the vendored platform workspace (ci.yml platform-test) must test the versions jc ships, which the
root Cargo.lock holds. Refresh: cp Cargo.lock vendor/joinedcontext-platform/ && cargo metadata
there (cargo drops what the workspace does not use).
  python3 lock-subset.py ROOT LOCK"""
import sys
import tomllib


def packages(path):
    return tomllib.load(open(path, "rb"))["package"]


root = {(p["name"], p["version"], p.get("source")) for p in packages(sys.argv[1])}
lock = packages(sys.argv[2])
off = [f"{p['name']} {p['version']}" for p in lock
       if p.get("source") and (p["name"], p["version"], p.get("source")) not in root]
print(f"{len(lock)} packages, {len(off)} outside {sys.argv[1]}" + (f": {', '.join(off)}" if off else ""))
sys.exit(1 if off else 0)
