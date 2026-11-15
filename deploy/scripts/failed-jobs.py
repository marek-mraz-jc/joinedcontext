#!/usr/bin/env python3
"""The CronJobs whose newest Job failed, from `kubectl get jobs -o json` on stdin (T-4385).

A harvest that dies in the night (a connection reset during a mesh outage) left two Jobs in Error
for ten hours and a catalogue a third full, with every smoke green. A CronJob keeps its failed
Jobs (`failedJobsHistoryLimit`) after a later run succeeds, so only the newest Job of each
CronJob counts. Prints one FAIL line per such CronJob and exits 1, else exits 0 silently.
"""
import json
import sys


def finished(status):
    """The Job's Complete or Failed condition that holds, or None while it runs or retries."""
    return next((c for c in status.get("conditions") or []
                 if c.get("type") in ("Complete", "Failed") and c.get("status") == "True"), None)


def failed(jobs):
    """(cronjob, job, reason) for every CronJob whose newest finished Job failed."""
    newest = {}
    for job in jobs.get("items") or []:
        meta, status = job.get("metadata") or {}, job.get("status") or {}
        owner = next((o.get("name") for o in meta.get("ownerReferences") or [] if o.get("kind") == "CronJob"), None)
        # finished is the Job's own Complete or Failed condition: `failed` counts failed pods,
        # which a Job retrying under its backoffLimit has too
        if owner is None or finished(status) is None:
            continue
        if owner not in newest or meta.get("creationTimestamp", "") > newest[owner][0]:
            newest[owner] = (meta.get("creationTimestamp", ""), meta.get("name"), status)
    out = []
    for owner, (_, name, status) in sorted(newest.items()):
        condition = finished(status)
        if condition.get("type") == "Complete":
            continue
        out.append((owner, name, condition.get("reason") or "Failed"))
    return out


def main():
    rows = failed(json.load(sys.stdin))
    for owner, name, reason in rows:
        print(f"  FAIL  CronJob {owner}: its newest Job {name} failed ({reason})")
    sys.exit(1 if rows else 0)


if __name__ == "__main__":
    main()
