#!/usr/bin/env python3
"""Verdict of dev/perf/heavy.sh (T-5898): exit 1 when the heavy workload failed a read, or (with a
baseline run) the broker's peak RSS or p95/p99 rose past the slack over the baseline's. The
absolute RSS target (200 MB, the memory budget) is reported, not gated, until the large surface
pages stream (later/antares-stream-large-surface-pages.md): today's per-request working memory
holds 390-490 MB under this load on every build. Without a baseline run the target is the gate.

  python3 dev/perf/heavy.py --results results/heavy [--rss-target-mb 200] [--slack 1.10]
  python3 dev/perf/heavy.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path


def _metric(summary: dict, name: str, stat: str) -> float:
    return float(summary.get("metrics", {}).get(name, {}).get(stat, 0) or 0)


def _pool(metrics_text: str) -> dict[str, float]:
    """The pool's own counters from the broker's /q/metrics after the run."""
    out: dict[str, float] = {}
    for line in metrics_text.splitlines():
        m = re.match(r"(antares_answer_pool_refusals_total|antares_answer_bytes_(?:sum|count))(\{[^}]*\})? (\S+)", line)
        if m:
            out[m.group(1)] = out.get(m.group(1), 0.0) + float(m.group(3))
    return out


def judge(results: Path, rss_target_mb: float, slack: float) -> tuple[list[str], list[str]]:
    """The report lines and the breaches."""
    lines = ["| build | reads | failed | 503 | p95 ms | p99 ms | max ms | peak RSS MB | writes ok |", "|---|---|---|---|---|---|---|---|---|"]
    breaches: list[str] = []
    runs: dict[str, dict] = {}
    for build in ("base", "head"):
        path = results / f"{build}.json"
        if not path.exists():
            continue
        s = json.loads(path.read_text())
        res_path = results / f"{build}.res"
        res = json.loads(res_path.read_text()) if res_path.exists() else {}
        run = {
            "reads": _metric(s, "heavy_reads", "count"),
            "failed": _metric(s, "heavy_failed", "count"),
            "overloaded": _metric(s, "heavy_overloaded", "count"),
            "p95": _metric(s, "heavy_ms", "p(95)"),
            "p99": _metric(s, "heavy_ms", "p(99)"),
            "max": _metric(s, "heavy_ms", "max"),
            "rss": float(res.get("broker", {}).get("rss_mb", 0)),
            "writes": _metric(s, "heavy_write_ok", "value"),
        }
        runs[build] = run
        lines.append(
            f"| {build} | {run['reads']:.0f} | {run['failed']:.0f} | {run['overloaded']:.0f} | {run['p95']:.0f} "
            f"| {run['p99']:.0f} | {run['max']:.0f} | {run['rss']:.1f} | {run['writes']:.3f} |"
        )
    head = runs.get("head")
    if head is None:
        return lines, ["head: no k6 summary (the run did not happen)"]
    if head["reads"] == 0:
        breaches.append("head: no read was made")
    if head["failed"] or head["overloaded"]:
        breaches.append(f"head: {head['failed']:.0f} failed reads, {head['overloaded']:.0f} of them 503")
    base = runs.get("base")
    if not head["rss"]:
        breaches.append("head: no RSS sample")
    elif head["rss"] > rss_target_mb:
        miss = f"head: peak RSS {head['rss']:.1f} MB > {rss_target_mb:.0f} MB target"
        if base and base["rss"]:
            lines.append(f"\ntarget missed (reported, not gated): {miss[6:]}")
        else:
            breaches.append(miss)
    if base:
        if base["rss"] and head["rss"] > base["rss"] * slack:
            breaches.append(f"head: peak RSS {head['rss']:.1f} MB > {slack:.2f} x baseline {base['rss']:.1f} MB")
        for stat in ("p95", "p99"):
            if base[stat] and head[stat] > base[stat] * slack:
                breaches.append(f"head: {stat} {head[stat]:.0f} ms > {slack:.2f} x baseline {base[stat]:.0f} ms")
    metrics = results / "head.metrics"
    if metrics.exists():
        pool = _pool(metrics.read_text())
        n = pool.get("antares_answer_bytes_count", 0)
        if n:
            lines.append(f"\nbuffered answers: {n:.0f}, mean {pool.get('antares_answer_bytes_sum', 0) / n / 1048576:.2f} MiB; pool refusals {pool.get('antares_answer_pool_refusals_total', 0):.0f}")
    return lines, breaches


def selftest() -> None:
    def write(d: Path, build: str, failed: int, p99: float, rss: float) -> None:
        metrics = {"heavy_reads": {"count": 100}, "heavy_ms": {"p(95)": p99 * 0.8, "p(99)": p99, "max": p99 * 2}, "heavy_write_ok": {"value": 1}}
        if failed:
            metrics["heavy_failed"] = {"count": failed}
            metrics["heavy_overloaded"] = {"count": failed}
        (d / f"{build}.json").write_text(json.dumps({"metrics": metrics}))
        (d / f"{build}.res").write_text(json.dumps({"broker": {"rss_mb": rss}}))

    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        write(d, "base", 0, 1000, 150)
        write(d, "head", 0, 1050, 160)
        assert judge(d, 200, 1.10)[1] == [], "a run within every bound is green"
        write(d, "head", 3, 1050, 160)
        assert any("3 failed" in b for b in judge(d, 200, 1.10)[1]), "a 503 is red"
        write(d, "head", 0, 1200, 160)
        assert any("p99" in b for b in judge(d, 200, 1.10)[1]), "a p99 20 % over the baseline is red"
        write(d, "head", 0, 1000, 250)
        assert any("baseline 150.0 MB" in b for b in judge(d, 200, 1.10)[1]), "RSS 10 % over the baseline is red"
        write(d, "base", 0, 1000, 460)
        write(d, "head", 0, 1000, 480)
        lines, breaches = judge(d, 200, 1.10)
        assert breaches == [], "RSS over the target but within the baseline's slack is green"
        assert any("target missed" in line for line in lines), "the missed target is reported"
        (d / "base.json").unlink()
        assert any("200 MB target" in b for b in judge(d, 200, 1.10)[1]), "without a baseline the target is the gate"
        write(d, "base", 0, 1000, 150)
        (d / "head.json").unlink()
        assert judge(d, 200, 1.10)[1], "a missing run is red"
    print("heavy.py selftest ok")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", type=Path)
    ap.add_argument("--rss-target-mb", type=float, default=200.0)
    ap.add_argument("--slack", type=float, default=1.10)
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        selftest()
        return 0
    if a.results is None:
        ap.error("--results is required")
    lines, breaches = judge(a.results, a.rss_target_mb, a.slack)
    print("\n".join(lines))
    for b in breaches:
        print(f"BREACH {b}")
    print("heavy: " + ("red" if breaches else "green"))
    return 1 if breaches else 0


if __name__ == "__main__":
    sys.exit(main())
