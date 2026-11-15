#!/usr/bin/env python3
"""The dev-smoke check of a full disk, from `df -P <path>` on stdin (T-4414, OPS-27).

The broker's history grew ~30 GB a week on the postgres data volume and nothing deleted it; the
first sign was a supervisor's df at 82 %, every smoke green. `--max` is the percentage above which
the volume fails the smoke. Prints one FAIL line per filesystem over it and exits 1, a line nothing
can be read from fails the same way, else exits 0 silently.
"""
import argparse
import sys


def used(lines):
    """(mount, percent used) per filesystem row of `df -P`; the header and blank lines skipped."""
    out = []
    for line in lines:
        cols = line.split()
        if len(cols) < 6 or not cols[4].endswith("%") or not cols[4][:-1].isdigit():
            continue
        out.append((cols[5], int(cols[4][:-1])))
    return out


def main():
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--max", type=int, required=True, help="the highest healthy percentage used")
    args.add_argument("--label", required=True, help="what the volume holds, for the FAIL line")
    opts = args.parse_args()
    rows = used(sys.stdin.read().splitlines())
    if not rows:
        print(f"  FAIL  {opts.label}: no df reading")
        sys.exit(1)
    over = [(mount, pct) for mount, pct in rows if pct > opts.max]
    for mount, pct in over:
        print(f"  FAIL  {opts.label} ({mount}) is {pct} % full, above {opts.max} %")
    sys.exit(1 if over else 0)


if __name__ == "__main__":
    main()
