#!/usr/bin/env python3
"""List the metrics an alert or recording rule or a Grafana dashboard reads that have no live
series in Prometheus (T-5925).

Run it before and after a change to what Prometheus scrapes and compare: a metric that is new in
the second list lost its data. Read-only: two GET requests to the Prometheus HTTP API.

    kubectl -n dev port-forward pod/prometheus-prometheus-0 42061:9090 &
    scripts/prom-read-metrics-live.py http://localhost:42061 > before.txt
    ... apply ...
    scripts/prom-read-metrics-live.py http://localhost:42061 | diff before.txt -

A name that never had a series (an HPA metric where no HPA exists) is in both lists; only metric
names Prometheus has stored at least once within retention are considered, so label names and
functions in the queries drop out."""

import json
import pathlib
import re
import sys
import urllib.parse
import urllib.request

DASHBOARDS = pathlib.Path(__file__).resolve().parents[1] / "components/grafana/charts/dashboards/files"
NAME = re.compile(r"[a-zA-Z_:][a-zA-Z0-9_:]*")


def get(base, path, **params):
    url = f"{base.rstrip('/')}/api/v1/{path}?{urllib.parse.urlencode(params)}"
    with urllib.request.urlopen(url, timeout=60) as answer:
        return json.load(answer)["data"]


def exprs(node):
    if isinstance(node, dict):
        for key, value in node.items():
            yield from [value] if key == "expr" and isinstance(value, str) else exprs(value)
    elif isinstance(node, list):
        for item in node:
            yield from exprs(item)


def main(base):
    queries = [r["query"] for g in get(base, "rules")["groups"] for r in g["rules"]]
    for path in sorted(DASHBOARDS.glob("*.json")):
        queries += list(exprs(json.loads(path.read_text())))
    read = {n for q in queries for n in NAME.findall(q)}
    stored = set(get(base, "label/__name__/values"))
    live = {s["metric"]["__name__"] for s in get(base, "query", query='group by (__name__) ({__name__=~".+"})')["result"]}
    for name in sorted((read & stored) - live):
        print(name)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: prom-read-metrics-live.py <prometheus base url>")
    main(sys.argv[1])
