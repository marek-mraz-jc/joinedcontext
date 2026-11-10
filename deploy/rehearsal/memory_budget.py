#!/usr/bin/env python3
"""memory_budget.py <memwatch.csv> [--after-min 12] [--idle-mib 1700] [--peak-mib 2400]

The memory budget of one jc on the prod rehearsal (jc-2gib, owner 2026-11-10: a whole deployment
under 2 GiB): reads a memwatch.py run (fresh start, prod import, pipelines on schedule) and fails
when jc's resident memory after the start window passes the bounds. Resident is VmRSS + VmSwap:
under host swap VmRSS alone reads low. Idle is the median after --after-min, peak the maximum.

The bounds are what jc holds with the assistant on the shared embedder (deployment
components/embedder, jc's [assistant] embedder_url): 1,502 MiB median and 2,290 MiB max measured
(evidence/jc-2gib, run R), against 2,181 / 2,864 MiB with e5-small and ONNX Runtime in the process
(run L2). Exit 0 inside both bounds, 1 past either, 2 on a run too short to judge."""

from __future__ import annotations

import argparse
import csv
import statistics
import sys
from datetime import datetime

IDLE_MIB = 1700
PEAK_MIB = 2400


def resident_after(rows: list[dict], after_min: float) -> list[float]:
    """VmRSS + VmSwap in MiB of every sample at least `after_min` after the first."""
    rows = [r for r in rows if r.get("VmRSS")]
    if not rows:
        return []
    start = datetime.fromisoformat(rows[0]["ts"].rstrip("Z"))
    return [
        (int(r["VmRSS"]) + int(r.get("VmSwap") or 0)) / 1024
        for r in rows
        if (datetime.fromisoformat(r["ts"].rstrip("Z")) - start).total_seconds() >= after_min * 60
    ]


def verdict(values: list[float], idle_mib: float, peak_mib: float) -> tuple[int, str]:
    if len(values) < 10:
        return 2, f"{len(values)} samples after the start window: run longer"
    idle, peak = statistics.median(values), max(values)
    line = f"idle {idle:.0f} MiB (bound {idle_mib:.0f}), peak {peak:.0f} MiB (bound {peak_mib:.0f})"
    return (0 if idle <= idle_mib and peak <= peak_mib else 1), line


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("csv")
    ap.add_argument("--after-min", type=float, default=12)
    ap.add_argument("--idle-mib", type=float, default=IDLE_MIB)
    ap.add_argument("--peak-mib", type=float, default=PEAK_MIB)
    a = ap.parse_args(argv)
    with open(a.csv, newline="") as f:
        code, line = verdict(resident_after(list(csv.DictReader(f)), a.after_min), a.idle_mib, a.peak_mib)
    print(("within budget: " if code == 0 else "OVER BUDGET: " if code == 1 else "no verdict: ") + line)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
