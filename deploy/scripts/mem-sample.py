#!/usr/bin/env python3
"""Memory use per container over time, and requests right-sized to it (T-5282).

  scripts/mem-sample.py sample samples.csv            # one `kubectl top --containers` row set, appended
  scripts/mem-sample.py loop samples.csv 300 288      # every 300 s, 288 times (24 h)
  scripts/mem-sample.py report samples.csv [--min-mi 32]
  scripts/mem-sample.py age http://localhost:9090 portal-portal [dev] [--days 3]

`age` reads the cluster's Prometheus (kubectl port-forward prometheus-prometheus-0 9090) and prints each
pod's anonymous RSS at fixed ages since its start: an allocator or code change compared at equal pod age
on the same cluster, from history, with no sampling loop (T-5283).

`report` reads the pod specs (kubectl get pods -A -o json) for the requests and prints per workload and
container: samples, p95 and max of the working set, the request, and p95 + 20 % rounded up to 16 Mi.
Pods of one Deployment/StatefulSet are one workload (the pod hash is stripped). KUBECONFIG picks the
cluster; the script only reads.
"""

import csv
import json
import math
import re
import subprocess
import sys
import time
from datetime import UTC, datetime

# Kubernetes builds pod and ReplicaSet suffixes from this alphabet (apimachinery rand.SafeEncodeString).
POD_HASH = re.compile(r"-(?:[bcdfghjklmnpqrstvwxz2456789]{6,10}-[bcdfghjklmnpqrstvwxz2456789]{5}"
                      r"|[bcdfghjklmnpqrstvwxz2456789]{5}|\d+)$")


def workload(pod: str) -> str:
    return POD_HASH.sub("", pod)


def mi(quantity: str) -> float:
    """Kubernetes memory quantity to MiB."""
    units = {"Ki": 1 / 1024, "Mi": 1, "Gi": 1024, "Ti": 1024 * 1024,
             "k": 1e3 / 2**20, "M": 1e6 / 2**20, "G": 1e9 / 2**20, "": 1 / 2**20}
    m = re.fullmatch(r"(\d+(?:\.\d+)?)([KMGT]i|[kMG])?", quantity.strip())
    if not m:
        raise ValueError(f"not a memory quantity: {quantity!r}")
    return float(m.group(1)) * units[m.group(2) or ""]


def parse_top(text: str):
    """Rows of `kubectl top pods -A --containers --no-headers`: ns, pod, container, cpu, memory."""
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 5:
            ns, pod, container, _cpu, mem = parts
            yield ns, pod, container, round(mi(mem))


def sample(path: str) -> int:
    out = subprocess.run(["kubectl", "top", "pods", "-A", "--containers", "--no-headers"],
                         check=True, capture_output=True, text=True, timeout=120).stdout
    ts = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    rows = [(ts, *r) for r in parse_top(out)]
    with open(path, "a", newline="") as f:
        csv.writer(f).writerows(rows)
    return len(rows)


def p95(values):
    s = sorted(values)
    return s[max(0, math.ceil(0.95 * len(s)) - 1)]


def requests_of(pods_json: dict):
    """(ns, workload, container) -> request MiB (None when unset), from a pod list."""
    req = {}
    for pod in pods_json.get("items", []):
        ns, name = pod["metadata"]["namespace"], pod["metadata"]["name"]
        for c in pod["spec"].get("containers", []):
            r = (c.get("resources") or {}).get("requests", {}).get("memory")
            req[(ns, workload(name), c["name"])] = round(mi(r)) if r else None
    return req


def report(rows, req, min_mi=32):
    """Per (ns, workload, container): n, p95, max, request, suggested (p95 + 20 %, 16 Mi steps)."""
    use = {}
    for _ts, ns, pod, container, mem in rows:
        use.setdefault((ns, workload(pod), container), []).append(int(mem))
    out = []
    for key, vals in sorted(use.items()):
        p = p95(vals)
        suggested = max(min_mi, math.ceil(p * 1.2 / 16) * 16)
        out.append((*key, len(vals), p, max(vals), req.get(key), suggested))
    return out


AGES_H = (0.25, 0.5, 1, 2, 3, 5, 8, 12, 24)


def by_age(rss: dict, starts: dict, ages=AGES_H, slack_s=300):
    """Per pod, oldest first: (pod, image tail, start, [MiB at each age or None], max MiB).

    `rss` is a Prometheus query_range answer for container_memory_rss by (pod, image), `starts` an
    instant answer for kube_pod_start_time by pod. A pod without a start time is skipped."""
    start_of = {r["metric"]["pod"]: float(r["value"][1]) for r in starts["data"]["result"]}
    out = []
    for r in rss["data"]["result"]:
        pod = r["metric"]["pod"]
        start = start_of.get(pod)
        if start is None or not r["values"]:
            continue
        points = [(float(t) - start, float(v) / 2**20) for t, v in r["values"]]
        at = []
        for h in ages:
            near = [m for a, m in points if abs(a - h * 3600) <= slack_s]
            at.append(round(near[0]) if near else None)
        image = r["metric"].get("image", "").rsplit(":", 1)[-1][:12]
        out.append((pod, image, start, at, round(max(m for _, m in points))))
    return sorted(out, key=lambda row: row[2])


def prometheus(base: str, path: str, params: dict) -> dict:
    import urllib.parse
    import urllib.request
    url = f"{base.rstrip('/')}/api/v1/{path}?{urllib.parse.urlencode(params)}"
    with urllib.request.urlopen(url, timeout=60) as answer:
        return json.load(answer)


def main(argv):
    cmd, path = argv[1], argv[2]
    if cmd == "sample":
        print(sample(path), "rows")
    elif cmd == "loop":
        every, times = int(argv[3]), int(argv[4])
        for i in range(times):
            try:
                sample(path)
            except (subprocess.SubprocessError, OSError) as e:
                print(f"sample {i} failed: {e}", file=sys.stderr)
            if i + 1 < times:
                time.sleep(every)
    elif cmd == "report":
        min_mi = int(argv[argv.index("--min-mi") + 1]) if "--min-mi" in argv else 32
        pods = json.loads(subprocess.run(["kubectl", "get", "pods", "-A", "-o", "json"], check=True,
                                         capture_output=True, text=True, timeout=120).stdout)
        with open(path, newline="") as f:
            rows = list(csv.reader(f))
        print("| namespace | workload | container | samples | p95 Mi | max Mi | request Mi | p95+20 % Mi |")
        print("|---|---|---|---|---|---|---|---|")
        for r in report(rows, requests_of(pods), min_mi):
            print("| " + " | ".join("-" if v is None else str(v) for v in r) + " |")
    elif cmd == "age":
        container = argv[3]
        ns = argv[4] if len(argv) > 4 and not argv[4].startswith("--") else "dev"
        days = float(argv[argv.index("--days") + 1]) if "--days" in argv else 3
        now = time.time()
        sel = f'namespace="{ns}",container="{container}"'
        rss = prometheus(path, "query_range", {
            "query": f"max by (pod,image)(container_memory_rss{{{sel}}})",
            "start": now - days * 86400, "end": now, "step": 300})
        starts = prometheus(path, "query", {
            "query": f'max by (pod)(max_over_time(kube_pod_start_time{{namespace="{ns}"}}[{int(days) + 1}d]))',
            "time": now})
        print("| pod | image | start | " + " | ".join(f"{h} h" for h in AGES_H) + " | max |")
        print("|---" * (len(AGES_H) + 4) + "|")
        for pod, image, start, at, top in by_age(rss, starts):
            when = datetime.fromtimestamp(start, UTC).strftime("%m-%dT%H:%MZ")
            cells = ["-" if v is None else str(v) for v in at]
            print(f"| {pod} | {image} | {when} | " + " | ".join(cells) + f" | {top} |")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv if len(sys.argv) >= 3 else [sys.argv[0], "help", ""])
