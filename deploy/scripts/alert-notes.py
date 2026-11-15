#!/usr/bin/env python3
"""Alertmanager's alerts as one note line each, for the deployer (T-5104).

Reads `GET /api/v2/alerts` of Alertmanager on stdin (scripts/alert-watch.sh fetches it through a
`kubectl port-forward` to `alertmanager-operated:9093`)
and the state file of the previous run, prints one line per alert that started or resolved since,
and rewrites the state file. The job sends each line through `tasks/note "@deployer ..."`.

A line carries the alert's name, its severity, its summary and only the labels in KEEP: hosts,
surfaces, Endpoints, projects, pipelines, volumes, nodes. Nothing else from a series reaches a
note, so a label a future exporter adds (a user, a client address) never leaves the cluster.
"""
import json
import sys

KEEP = ("cluster", "cronjob", "domain", "endpoint", "host", "instance", "mountpoint", "namespace",
        "persistentvolumeclaim", "pipeline", "project", "route", "surface", "target")


def line(alert, state):
    labels = alert.get("labels") or {}
    kept = " ".join(f"{k}={labels[k]}" for k in KEEP if k in labels)
    summary = ((alert.get("annotations") or {}).get("summary") or "").replace("\n", " ")[:300]
    return f"{state} {labels.get('alertname', '?')} [{labels.get('severity', 'none')}] {kept}: {summary}".replace("  ", " ")


def notes(alerts, before):
    """(lines, now): `now` maps each active alert's fingerprint to its line, `before` is the last run's."""
    now = {}
    for alert in alerts:
        if (alert.get("status") or {}).get("state") == "active" and alert.get("fingerprint"):
            now[alert["fingerprint"]] = line(alert, "FIRING")
    lines = [now[f] for f in sorted(now) if f not in before]
    lines += ["RESOLVED" + before[f].removeprefix("FIRING") for f in sorted(before) if f not in now]
    return lines, now


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: alert-notes.py STATE_FILE < alerts.json")
    try:
        with open(sys.argv[1]) as f:
            before = json.load(f)
    except FileNotFoundError:
        before = {}
    lines, now = notes(json.load(sys.stdin), before)
    with open(sys.argv[1], "w") as f:
        json.dump(now, f)
    print("\n".join(lines))


if __name__ == "__main__":
    main()
