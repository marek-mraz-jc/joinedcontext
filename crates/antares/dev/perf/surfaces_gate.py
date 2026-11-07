#!/usr/bin/env python3
"""Judge a dev/perf/surfaces.sh run (T-4889): each shape's k6 summary against its budget in
dev/perf/surfaces-budgets.json and against the committed baseline, plus the CPU and RSS the
broker and PostgreSQL used while the shape ran.

    surfaces_gate.py sample --pid 1234 [--container pg] --out results/surfaces/vanilla-ngsi-q.res   (until killed)
    surfaces_gate.py judge --results results/surfaces [--baseline ci/perf-baseline/surfaces.json]
    surfaces_gate.py baseline --results results/surfaces --machine ccx33 > ci/perf-baseline/surfaces.json
    surfaces_gate.py --selftest

A shape fails on p95, p99 or error rate over its budget, on throughput under 95 % of its rate
(dropped iterations), and, when the baseline was measured on the same machine class
(PERF_MACHINE), on p99 more than `tolerance` above that baseline. A baseline from another machine
is reported beside the number and judges nothing: a laptop's p99 says nothing about a runner's.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BUDGETS = HERE / "surfaces-budgets.json"
TICKS = os.sysconf("SC_CLK_TCK")
PAGE_MB = os.sysconf("SC_PAGE_SIZE") / 2**20


def _proc(pid: int) -> tuple[int, float] | None:
    """(cpu ticks, rss MB) of one process, None when it is gone."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return int(stat[11]) + int(stat[12]), int(stat[21]) * PAGE_MB
    except (OSError, IndexError, ValueError):
        return None


_UNITS = {
    "B": 1 / 2**20,
    "KiB": 1 / 1024,
    "MiB": 1,
    "GiB": 1024,
    "kB": 1 / 1000,
    "MB": 1,
    "GB": 1000,
}


def _container(name: str) -> tuple[float, float] | None:
    """(CPU %, memory MB) of a container from `docker stats`: its cgroup, so PostgreSQL's shared
    buffers count once, which a sum of its processes' RSS does not."""
    try:
        out = subprocess.run(
            [
                "docker",
                "stats",
                "--no-stream",
                "--format",
                "{{.CPUPerc}} {{.MemUsage}}",
                name,
            ],
            capture_output=True,
            text=True,
            timeout=20,
            check=True,
        ).stdout.split()
        mem = re.fullmatch(r"([\d.]+)([A-Za-z]+)", out[1])
        return float(out[0].rstrip("%")), float(mem[1]) * _UNITS[mem[2]]
    except (
        OSError,
        subprocess.SubprocessError,
        IndexError,
        ValueError,
        KeyError,
        TypeError,
    ):
        return None


def sample(pid: int, out: Path, container: str | None, every: float = 1.0) -> None:
    """Sample the broker (and PostgreSQL's container) until SIGTERM; write peak RSS and mean CPU %."""
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(1))
    first = last = None
    peak = 0.0
    pg: list[tuple[float, float]] = []
    while not stop:
        now = time.monotonic()
        if (seen := _proc(pid)) is not None:
            first = first or (now, seen[0])
            last = (now, seen[0])
            peak = max(peak, seen[1])
        if container and (c := _container(container)) is not None:
            pg.append(c)
        time.sleep(every)
    res = {}
    if first and last and last[0] > first[0]:
        cpu = 100.0 * (last[1] - first[1]) / TICKS / (last[0] - first[0])
        res["broker"] = {"rss_mb": round(peak, 1), "cpu_pct": round(cpu, 1)}
    if pg:
        res["postgres"] = {
            "rss_mb": round(max(m for _, m in pg), 1),
            "cpu_pct": round(sum(c for c, _ in pg) / len(pg), 1),
        }
    out.write_text(json.dumps(res))


def _seconds(text: str) -> int:
    return int(text[:-1]) * {"s": 1, "m": 60}[text[-1]]


def measure(summary: dict, shape: str, rate: int, duration: str) -> dict:
    """The numbers a k6 --summary-export carries for one shape."""
    m = summary["metrics"]
    trend = (
        m.get("ws_fanout_ms") if shape == "ws-fanout" else m.get("http_req_duration")
    )
    ok = m.get("shape_ok", {})
    total = ok.get("passes", 0) + ok.get("fails", 0)
    # a fan-out's errors are the writes refused plus the sockets that never subscribed
    sub = (
        m.get("ws_subscribed", {"passes": 0, "fails": 0})
        if shape == "ws-fanout"
        else {"passes": 0, "fails": 0}
    )
    # and the notifications the writes owed that never arrived
    owed = m.get("ws_expected", {}).get("count", 0) if shape == "ws-fanout" else 0
    lost = max(0, owed - m.get("ws_notifications", {}).get("count", 0))
    fails = ok.get("fails", 0) + sub.get("fails", 0) + lost
    asked = total + sub.get("passes", 0) + sub.get("fails", 0) + owed
    return {
        "p95_ms": round((trend or {}).get("p(95)", float("nan")), 2),
        "p99_ms": round((trend or {}).get("p(99)", float("nan")), 2),
        "error_pct": round(100.0 * fails / asked, 3) if asked else 100.0,
        "rps": round(total / _seconds(duration), 1),
        "rate": rate,
        "dropped": int(m.get("dropped_iterations", {}).get("count", 0)),
    }


def verdict(
    shape: str, got: dict, budget: dict, base: dict | None, tolerance: float
) -> list[str]:
    """Every breach of one shape, empty when it holds its budget."""
    breaches = []
    for key in ("p95_ms", "p99_ms"):
        if not got[key] <= budget[key]:  # NaN (nothing measured) breaches too
            breaches.append(f"{key} {got[key]} > {budget[key]}")
    if got["error_pct"] > budget["max_error_pct"]:
        breaches.append(f"errors {got['error_pct']} % > {budget['max_error_pct']} %")
    if got["rps"] < 0.95 * budget["rate"]:
        breaches.append(f"throughput {got['rps']}/s < 95 % of {budget['rate']}/s")
    if base and got["p99_ms"] > base["p99_ms"] * (1 + tolerance):
        breaches.append(
            f"p99 {got['p99_ms']} > baseline {base['p99_ms']} + {tolerance:.0%}"
        )
    return breaches


def load_results(results: Path, budgets: dict) -> dict:
    """{build: {shape: numbers}} for every summary the run left in `results`."""
    out: dict = {}
    for build in ("vanilla", "platform"):
        for shape, budget in budgets["shapes"].items():
            path = results / f"{build}-{shape}.json"
            if build not in budget["builds"] or not path.exists():
                continue
            got = measure(
                json.loads(path.read_text()), shape, budget["rate"], budgets["duration"]
            )
            res = results / f"{build}-{shape}.res"
            got["resources"] = json.loads(res.read_text()) if res.exists() else {}
            out.setdefault(build, {})[shape] = got
    return out


def judge(
    results: Path, budgets: dict, baseline: dict | None, machine: str
) -> tuple[str, int]:
    """The markdown table of a run and how many shapes breached (a missing shape counts)."""
    runs = load_results(results, budgets)
    same = bool(baseline) and baseline.get("machine") == machine
    rows = [
        f"machine `{machine}`; baseline: "
        + (
            "none"
            if not baseline
            else f"`{baseline.get('machine')}`"
            + ("" if same else ", another machine: reported, not judged")
        ),
        "",
        "| build | shape | rate/s | rps | p95 ms | p99 ms | errors % | broker CPU % / RSS MB | postgres CPU % / RSS MB | baseline p99 | verdict |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    breached = 0
    for build in ("vanilla", "platform"):
        for shape, budget in budgets["shapes"].items():
            if build not in budget["builds"]:
                continue
            got = runs.get(build, {}).get(shape)
            if got is None:
                breached += 1
                rows.append(
                    f"| {build} | {shape} | {budget['rate']} | | | | | | | | FAIL: did not run |"
                )
                continue
            base = (baseline or {}).get("builds", {}).get(build, {}).get(shape)
            breaches = verdict(
                shape, got, budget, base if same else None, budgets["tolerance"]
            )
            breached += bool(breaches)
            r = got["resources"]
            rows.append(
                f"| {build} | {shape} | {budget['rate']} | {got['rps']} | {got['p95_ms']} | {got['p99_ms']} "
                f"| {got['error_pct']} | {_usage(r, 'broker')} | {_usage(r, 'postgres')} | {base['p99_ms'] if base else ''} "
                f"| {'FAIL: ' + '; '.join(breaches) if breaches else 'ok'} |"
            )
    return "\n".join(rows) + "\n", breached


def _usage(resources: dict, name: str) -> str:
    """`CPU % / RSS MB` of one process the sampler watched, empty when it watched none."""
    r = resources.get(name)
    return f"{r['cpu_pct']} / {r['rss_mb']}" if r else ""


def make_baseline(results: Path, budgets: dict, machine: str) -> dict:
    """The run's numbers of every shape that held its budget: a breach is no baseline to hold."""
    runs = load_results(results, budgets)
    keep = ("p95_ms", "p99_ms", "rps", "error_pct")
    return {
        "machine": machine,
        "measured": time.strftime("%Y-%m-%dT%H:%MZ", time.gmtime()),
        "builds": {
            b: {
                s: {k: v[k] for k in keep}
                for s, v in shapes.items()
                if not verdict(s, v, budgets["shapes"][s], None, 0)
            }
            for b, shapes in runs.items()
        },
    }


def selftest() -> None:
    """A planted slowdown, a planted error and a missing shape each fail; the clean run passes."""
    budgets = {
        "duration": "10s",
        "tolerance": 0.5,
        "shapes": {
            "ngsi-q": {
                "builds": ["vanilla"],
                "rate": 10,
                "p95_ms": 50,
                "p99_ms": 100,
                "max_error_pct": 0.1,
            }
        },
    }

    def summary(p95, p99, fails=0, passes=100):
        return {
            "metrics": {
                "http_req_duration": {"p(95)": p95, "p(99)": p99},
                "shape_ok": {"passes": passes, "fails": fails},
                "dropped_iterations": {"count": 0},
            }
        }

    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        f = d / "vanilla-ngsi-q.json"
        f.write_text(json.dumps(summary(10, 20)))
        assert judge(d, budgets, None, "x")[1] == 0, "a clean run passes"
        base = make_baseline(d, budgets, "x")
        assert base["builds"]["vanilla"]["ngsi-q"]["p99_ms"] == 20
        f.write_text(json.dumps(summary(60, 90)))
        assert make_baseline(d, budgets, "x")["builds"]["vanilla"] == {}, (
            "a breach is no baseline"
        )
        f.write_text(json.dumps(summary(10, 40)))
        assert judge(d, budgets, base, "x")[1] == 1, (
            "p99 doubled over a same-machine baseline fails"
        )
        assert judge(d, budgets, base, "y")[1] == 0, (
            "another machine's baseline judges nothing"
        )
        f.write_text(json.dumps(summary(60, 90)))
        table, n = judge(d, budgets, None, "x")
        assert n == 1 and "p95_ms 60 > 50" in table, "a planted slowdown breaks p95"
        f.write_text(json.dumps(summary(10, 20, fails=1, passes=99)))
        assert judge(d, budgets, None, "x")[1] == 1, "1 % errors over 0.1 % fails"
        f.write_text(json.dumps(summary(10, 20, passes=50)))
        assert judge(d, budgets, None, "x")[1] == 1, "half the rate fails"
        f.unlink()
        assert judge(d, budgets, None, "x")[1] == 1, "a shape that did not run fails"
    real = json.loads(BUDGETS.read_text())
    for shape, b in real["shapes"].items():
        assert b.get("reason"), f"{shape}: a budget without its reason"
        assert set(b["builds"]) <= {"vanilla", "platform"}, shape
    print("surfaces_gate selftest: ok")


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        selftest()
        return 0
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sample")
    s.add_argument("--pid", type=int, required=True)
    s.add_argument("--out", type=Path, required=True)
    s.add_argument("--container", help="PostgreSQL's container, read with docker stats")
    for name in ("judge", "baseline"):
        j = sub.add_parser(name)
        j.add_argument("--results", type=Path, required=True)
        j.add_argument("--budgets", type=Path, default=BUDGETS)
        j.add_argument("--machine", default=os.environ.get("PERF_MACHINE", "unknown"))
        if name == "judge":
            j.add_argument("--baseline", type=Path)
    a = p.parse_args()
    if a.cmd == "sample":
        sample(a.pid, a.out, a.container)
        return 0
    budgets = json.loads(a.budgets.read_text())
    if a.cmd == "baseline":
        print(json.dumps(make_baseline(a.results, budgets, a.machine), indent=2))
        return 0
    baseline = (
        json.loads(a.baseline.read_text())
        if a.baseline and a.baseline.exists()
        else None
    )
    table, breached = judge(a.results, budgets, baseline, a.machine)
    print(table)
    return 1 if breached else 0


if __name__ == "__main__":
    sys.exit(main())
