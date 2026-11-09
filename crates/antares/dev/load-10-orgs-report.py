# SPDX-License-Identifier: Apache-2.0
"""Report of one dev/load-10-orgs.sh run (T-6628): the k6 summary, broker metrics, docker stats
and pg_stat_statements of its OUT directory as one Markdown table set.

    python3 dev/load-10-orgs-report.py results/load-10-orgs/<stamp> > report.md
"""

import json
import re
import sys
from pathlib import Path

KINDS = ["query", "get", "temporal", "upsert", "xtenant"]


def seconds(text):
    m = re.fullmatch(r"(\d+(?:\.\d+)?)(ms|s|m)", text)
    if not m:
        raise ValueError(f"duration {text!r}")
    return float(m[1]) * {"ms": 0.001, "s": 1, "m": 60}[m[2]]


def prom(path):
    """Sum of every sample per metric name (labels folded), from a Prometheus text page."""
    out = {}
    if not path.exists():
        return out
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        name_labels, _, value = line.rpartition(" ")
        name = name_labels.split("{", 1)[0]
        try:
            out[name] = out.get(name, 0.0) + float(value)
        except ValueError:
            pass
    return out


def ms(v):
    return f"{v:.1f}" if v is not None else "-"


def main(out_dir):
    out = Path(out_dir)
    data = json.loads((out / "summary.json").read_text())
    metrics = data["metrics"]
    meta = (out / "meta.txt").read_text()
    duration = seconds(re.search(r"duration=(\S+)", meta)[1])

    def vals(name):
        m = metrics.get(name)
        return (m or {}).get("values", m or {})

    def trend(sel):
        v = vals(f"http_req_duration{{{sel}}}")
        return v.get("med"), v.get("p(95)"), v.get("p(99)")

    def reqs(sel):
        v = vals(f"http_req_failed{{{sel}}}")
        return int(v.get("passes", 0) + v.get("fails", 0)), v.get("passes", 0)

    lines = ["# load-10-orgs run", "", "```", meta.strip(), "```", ""]
    total = vals("http_reqs{phase:load}").get("count", 0)
    dropped = vals("dropped_iterations").get("count", 0)
    lines += [
        "## 1. Throughput",
        "",
        f"- achieved: **{total / duration:.0f} req/s** ({int(total)} requests in the {duration:.0f} s measured window)",
        f"- dropped iterations (k6 had no free VU at the arrival time, warm-up included): {int(dropped)}",
        "",
        "## 2. Latency per request kind (ms, measured window)",
        "",
        "| kind | requests | failed | p50 | p95 | p99 |",
        "|---|---|---|---|---|---|",
    ]
    for k in KINDS:
        n, failed = reqs(f"phase:load,kind:{k}")
        p50, p95, p99 = trend(f"phase:load,kind:{k}")
        lines.append(
            f"| {k} | {n} | {int(failed)} | {ms(p50)} | {ms(p95)} | {ms(p99)} |"
        )
    lines.append("")
    lines.append(
        "`failed` counts k6's unexpected statuses: a 2xx is expected everywhere but `xtenant`, where only 403 and 404 are."
    )

    tenants = sorted(
        {re.search(r"tenant:(t\d+)", k)[1] for k in metrics if "tenant:t" in k},
        key=lambda t: int(t[1:]),
    )
    lines += [
        "",
        "## 3. Latency per Tenant (ms, measured window)",
        "",
        "| Tenant | requests | failed | p50 | p95 | p99 | p99 query | p99 get | p99 temporal | p99 upsert |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    p99s = {}
    for t in tenants:
        n, failed = reqs(f"phase:load,tenant:{t}")
        p50, p95, p99 = trend(f"phase:load,tenant:{t}")
        p99s[t] = p99
        per_kind = [ms(trend(f"phase:load,tenant:{t},kind:{k}")[2]) for k in KINDS[:4]]
        lines.append(
            f"| {t} | {n} | {int(failed)} | {ms(p50)} | {ms(p95)} | {ms(p99)} | "
            + " | ".join(per_kind)
            + " |"
        )
    ranked = [(p, t) for t, p in p99s.items() if p is not None]
    if ranked:
        worst, best = max(ranked), min(ranked)
        lines += [
            "",
            f"Fairness: worst Tenant p99 {worst[1]} {worst[0]:.1f} ms vs best {best[1]} {best[0]:.1f} ms (x{worst[0] / best[0]:.2f}).",
        ]

    lines += [
        "",
        "## 4. Statuses (whole run, warm-up included)",
        "",
        "| status | count |",
        "|---|---|",
    ]
    for k in sorted(k for k in metrics if k.startswith("status_")):
        c = vals(k).get("count", 0)
        if c:
            lines.append(f"| {k[7:]} | {int(c)} |")

    leaks = vals("leaks").get("count", 0)
    unexpected = vals("xtenant_unexpected").get("count", 0)
    xt, _ = reqs("phase:load,kind:xtenant")
    lines += [
        "",
        "## 5. Isolation",
        "",
        f"- leaks (an id of another Tenant in an answer, or a 2xx with data on a cross-Tenant read): **{int(leaks)}**",
        f"- cross-Tenant reads with the own caller in the measured window: {xt}; answered other than 403/404: {int(unexpected)}",
        "",
    ]

    before, after = prom(out / "metrics-before.txt"), prom(out / "metrics-after.txt")

    def delta(name):
        return after.get(name, 0.0) - before.get(name, 0.0)

    http_n = delta("antares_http_request_duration_seconds_count")
    http_s = delta("antares_http_request_duration_seconds_sum")
    begin_n = delta("antares_pg_transaction_begin_seconds_count")
    begin_s = delta("antares_pg_transaction_begin_seconds_sum")
    pg_total = (
        (out / "pg-total.txt").read_text().strip().split("|")
        if (out / "pg-total.txt").exists()
        else ["0", "0"]
    )
    pg_ms = float(pg_total[0] or 0)
    pg_calls = int(float(pg_total[1] or 0))
    cpu = {"broker": [], "pg": []}
    mem = {"broker": [], "pg": []}
    for row in (
        (out / "docker-stats.txt").read_text().splitlines()
        if (out / "docker-stats.txt").exists()
        else []
    ):
        for name, c, m, unit in re.findall(
            r"load10-(broker|pg)-\d+ cpu=([\d.]+)% mem=([\d.]+)([KMG]iB)", row
        ):
            cpu[name].append(float(c))
            mem[name].append(float(m) * {"KiB": 1 / 1024, "MiB": 1, "GiB": 1024}[unit])
    conns = []
    for row in (
        (out / "pg-connections.txt").read_text().splitlines()
        if (out / "pg-connections.txt").exists()
        else []
    ):
        parts = row.split("|")
        if len(parts) == 3:
            conns.append((int(parts[1]), int(parts[2])))

    def peak(xs):
        return f"{max(xs):.0f}" if xs else "-"

    def mean(xs):
        return f"{sum(xs) / len(xs):.0f}" if xs else "-"

    lines += [
        "## 6. Resources and stages",
        "",
        "| measure | value |",
        "|---|---|",
        f"| broker CPU % (docker stats, mean / peak; 100 = one core) | {mean(cpu['broker'])} / {peak(cpu['broker'])} |",
        f"| PostGIS CPU % (mean / peak) | {mean(cpu['pg'])} / {peak(cpu['pg'])} |",
        f"| broker memory MiB (cgroup, peak) | {peak(mem['broker'])} |",
        f"| broker resident / allocated MiB at the end (`antares_memory_*`) | {after.get('antares_memory_resident_bytes', 0) / 2**20:.0f} / {after.get('antares_memory_allocated_bytes', 0) / 2**20:.0f} |",
        f"| PostGIS connections of the broker (peak total / peak active) | {max((c[0] for c in conns), default=0)} / {max((c[1] for c in conns), default=0)} |",
        f"| broker service time, mean ms (`antares_http_request_duration_seconds`) | {1000 * http_s / http_n if http_n else 0:.2f} over {int(http_n)} requests |",
        f"| pool wait + BEGIN, mean ms (`antares_pg_transaction_begin_seconds`) | {1000 * begin_s / begin_n if begin_n else 0:.2f} over {int(begin_n)} transactions |",
        f"| pool acquire timeouts (503) | {int(delta('antares_pg_pool_timeouts_total'))} |",
        f"| PostgreSQL execution time, total s (`pg_stat_statements`, measured run incl. warm-up) | {pg_ms / 1000:.1f} over {pg_calls} statements |",
        f"| broker service time, total s | {http_s:.1f} |",
        "",
        "PostgreSQL execution time over broker service time is the share of a request spent executing SQL; "
        + "the rest is pool wait, round trips and the broker's own work (parse, policy, serialise).",
        f"Share: {100 * (pg_ms / 1000) / http_s if http_s else 0:.0f} %.",
        "",
        "## 7. Slowest statements (pg_stat_statements, by total time)",
        "",
        "```",
        "\n".join((out / "pg_stat_statements.txt").read_text().splitlines()[:12])
        if (out / "pg_stat_statements.txt").exists()
        else "",
        "```",
    ]
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1])
