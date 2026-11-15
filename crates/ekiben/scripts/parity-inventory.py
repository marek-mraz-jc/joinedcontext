#!/usr/bin/env python3
"""Inventory gate: what Bento 1.22 has that ekiben lacks, refuses by policy, or adds (G-01).

Both binaries answer `list --format json`; every name of the kinds below lands in exactly one
list of tests/parity/missing.json: missing (Bento has it, ekiben lacks it), refused (policy, with
its reason), extension (ekiben only, with its reason). Reasons live in scripts/parity-policy.json,
whose `deprecated` section names what Bento still runs but leaves out of its listing (`meta`,
the `sql` output): ekiben carrying one of those is parity, not an extension.

    parity-inventory.py --record                 record .bento/bento's listing in
                                                 tests/parity/bento-list.json (scripts/reference-bento.sh)
    parity-inventory.py --ekiben PATH            write tests/parity/missing.json
    parity-inventory.py --ekiben PATH --check    fail on any name newly missing, on an ekiben-only
                                                 name without a reason, or on a stale policy entry

Exit 1 names every offending entry. Bento's side is the recorded listing, so CI needs no Bento binary.
"""
import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KINDS = ["inputs", "outputs", "processors", "caches", "rate-limits", "buffers", "scanners",
         "bloblang-functions", "bloblang-methods"]


def names(data, source):
    absent = [k for k in KINDS if k not in data]
    if absent:
        sys.exit(f"{source} has no {', '.join(absent)}")
    return {f"{k}/{n}" for k in KINDS for n in data[k]}


def listing(binary):
    out = subprocess.run([binary, "list", "--format", "json"], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bento", default=os.path.join(ROOT, ".bento", "bento"))
    ap.add_argument("--recorded", default=os.path.join(ROOT, "tests", "parity", "bento-list.json"))
    ap.add_argument("--record", action="store_true")
    ap.add_argument("--ekiben")
    ap.add_argument("--policy", default=os.path.join(ROOT, "scripts", "parity-policy.json"))
    ap.add_argument("--out", default=os.path.join(ROOT, "tests", "parity", "missing.json"))
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()

    if a.record:
        version = subprocess.run([a.bento, "--version"], capture_output=True, text=True, check=True)
        data = {k: sorted(v) for k, v in listing(a.bento).items() if k in KINDS}
        data["version"] = version.stdout.splitlines()[0]
        with open(a.recorded, "w") as f:
            json.dump(data, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"recorded {data['version']} -> {a.recorded}")
        return 0
    if not a.ekiben:
        ap.error("--ekiben PATH is required unless --record")
    with open(a.recorded) as f:
        recorded = json.load(f)
    bento = names(recorded, a.recorded)
    ekiben = names(listing(a.ekiben), f"{a.ekiben} list --format json")
    with open(a.policy) as f:
        policy = json.load(f)
    refused, ext = policy.get("refused", {}), policy.get("extension", {})
    deprecated = policy.get("deprecated", {})
    errors = []
    for section, entries in (("refused", refused), ("extension", ext), ("deprecated", deprecated)):
        for name, why in entries.items():
            if not isinstance(why, str) or not why.strip():
                errors.append(f"{section} {name}: no reason given")
            if name.split("/", 1)[0] not in KINDS:
                errors.append(f"{section} {name}: kind is not one of {', '.join(KINDS)}")
    errors += [f"refused {n}: Bento has no such name (stale policy entry)" for n in refused if n not in bento]
    errors += [f"extension {n}: not ekiben-only (stale policy entry)" for n in ext if n in bento or n not in ekiben]
    errors += [f"deprecated {n}: listed by Bento or absent from ekiben (stale policy entry)"
               for n in deprecated if n in bento or n not in ekiben]

    missing = sorted(bento - ekiben - refused.keys())
    extension = sorted(ekiben - bento - deprecated.keys())
    errors += [f"{n}: ekiben has it and Bento does not; list it under extension with a reason"
               for n in extension if n not in ext]

    report = {
        "bento": recorded.get("version", ""),
        "missing": missing,
        "refused": [{"name": n, "reason": refused[n]} for n in sorted(refused) if n in bento],
        "extension": [{"name": n, "reason": ext[n]} for n in extension if n in ext],
    }

    if a.check:
        with open(a.out) as f:
            committed = json.load(f)
        errors += [f"{n}: Bento has it, ekiben no longer does (not listed as missing)"
                   for n in missing if n not in set(committed.get("missing", []))]
        ported = sorted(set(committed.get("missing", [])) - set(missing))
        if ported:
            print(f"now present in ekiben, regenerate {a.out}: {', '.join(ported)}")
    else:
        os.makedirs(os.path.dirname(a.out), exist_ok=True)
        with open(a.out, "w") as f:
            json.dump(report, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"{len(missing)} missing, {len(report['refused'])} refused, "
              f"{len(report['extension'])} extensions -> {a.out}")

    for e in errors:
        print("FAIL", e)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
