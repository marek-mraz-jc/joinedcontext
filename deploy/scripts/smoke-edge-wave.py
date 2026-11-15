#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Make the Portal push one edge wave and check that it reached the served Secret (T-5318).

    scripts/smoke-edge-wave.py [--timeout SECONDS]

The rest of the smoke reads the edge APISIX serves, which stays the same when the Portal's write
of Secret apisix-standalone-config is refused (prod, 2026-10-20: a ValidatingAdmissionPolicy
answered 422 for hours, every smoke green). This step takes the Portal's marker
`joinedcontext.com/composed-by: portal` off that Secret and nothing else: the file APISIX serves
is left as it is. The Portal's leader composes the edge file on every sync (reconciler/daemon.rs
converge_edge_file); finding the marker gone, it writes the same file back with the marker
(reconciler/edge_file.rs converge). ok: the marker is back on a newer resourceVersion and no
Portal pod logged "edge file not written" since the step began. ko: the timeout passes, or the
Portal logged that line, or two leader sync runs finished since the step began without the
write; the step then puts the marker back itself, because a helm sync of the apisix configuration
chart rewrites the base over the Apps' routes while the marker is missing.

The wait follows the Portal's own "sync run finished" lines of the last hour (T-5796): twice the
longest leader run plus the sync interval, between 300 s and 1800 s. A leader run took 174-623 s
on prod (3360 manifests, 2026-10-26) and 70-364 s on dev, so a fixed 300 s failed with no write
refused.
"""

import argparse
import json
import re
import signal
import subprocess
import sys
import time
from datetime import UTC, datetime

SECRET = "apisix-standalone-config"
MARKER = "joinedcontext.com/composed-by"
MARKED = "portal"
# `!run` skips the probe pods smoke.sh starts with `kubectl run` wearing the Portal label (the meshed
# forged-caller probe): one still initializing failed `kubectl logs -l` (stage smoke 2026-11-03).
PORTAL_LABEL = "app.kubernetes.io/name=portal-portal,!run"
REFUSED = "edge file not written"
SYNCED = "sync run finished"
# JC_PORTAL_SYNC_INTERVAL: the pause between two sync runs.
INTERVAL = 60
FLOOR, CEILING = 300.0, 1800.0
ELAPSED = re.compile(r'"?elapsed_ms"?[=:]\s*(\d+)')
LEADER = re.compile(r'"?leader"?[=:]\s*true')


def kubectl(*args: str) -> str:
    done = subprocess.run(["kubectl", *args], capture_output=True, text=True, timeout=60, check=False)
    if done.returncode != 0:
        raise RuntimeError(f"kubectl {' '.join(args[:3])}: {done.stderr.strip()[:300]}")
    return done.stdout


def served() -> dict:
    items = json.loads(kubectl("get", "secrets", "-A", "--field-selector", f"metadata.name={SECRET}", "-o", "json"))["items"]
    if len(items) != 1:
        raise RuntimeError(f"{len(items)} Secrets named {SECRET} in the cluster, expected one in the APISIX namespace")
    return items[0]["metadata"]


def marked(meta: dict) -> bool:
    return (meta.get("annotations") or {}).get(MARKER) == MARKED


def portal_log(window: str) -> list[str]:
    """Every Portal pod's log lines in `window` (a kubectl --since or --since-time flag)."""
    namespaces = kubectl("get", "pods", "-A", "-l", PORTAL_LABEL, "-o", "jsonpath={.items[*].metadata.namespace}").split()
    if not namespaces:
        raise RuntimeError(f"no Portal pod ({PORTAL_LABEL}) in the cluster")
    lines = []
    for namespace in sorted(set(namespaces)):
        lines += kubectl("logs", "-n", namespace, "-l", PORTAL_LABEL, "--all-containers=true",
                         window, "--tail=-1", "--max-log-requests=10").splitlines()
    return lines


def refusals(lines: list[str]) -> list[str]:
    """The Portal's "edge file not written" lines."""
    return [line.strip()[:400] for line in lines if REFUSED in line]


def leader_runs(lines: list[str]) -> list[float]:
    """Seconds of each sync run a leader finished: only the leader converges the edge file."""
    runs = []
    for line in lines:
        elapsed = ELAPSED.search(line)
        if SYNCED in line and LEADER.search(line) and elapsed:
            runs.append(int(elapsed.group(1)) / 1000)
    return runs


def measured_timeout(lines: list[str]) -> tuple[float, str]:
    """Two leader runs and the interval between them, from the runs `lines` hold."""
    runs = leader_runs(lines)
    if not runs:
        return CEILING, "no leader sync run in the Portal log of the last hour"
    longest = max(runs)
    timeout = min(CEILING, max(FLOOR, 2 * longest + INTERVAL))
    return timeout, f"2 x the longest of {len(runs)} leader sync runs in the last hour ({longest:.0f} s) + {INTERVAL} s"


def restore(namespace: str) -> None:
    if not marked(served()):
        kubectl("annotate", "secret", "-n", namespace, SECRET, f"{MARKER}={MARKED}", "--overwrite")


def wave(timeout: float | None = None, poll: float = 5.0, clock=time.monotonic, sleep=time.sleep) -> tuple[bool, str]:
    meta = served()
    namespace, before = meta["namespace"], meta["resourceVersion"]
    if not marked(meta):
        return False, f"{namespace}/{SECRET} carries no {MARKER}: {MARKED}, the Portal never composed the edge file"
    basis = "--timeout"
    if timeout is None:
        timeout, basis = measured_timeout(portal_log("--since=1h"))
    since = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    # --resource-version: a Portal write between the read and this patch fails it, not lost.
    kubectl("annotate", "secret", "-n", namespace, SECRET, f"{MARKER}-", f"--resource-version={before}")
    deadline = clock() + timeout
    try:
        while True:
            # The log before the Secret: a write that landed before a run's end line is seen.
            lines = portal_log(f"--since-time={since}")
            refused = refusals(lines)
            if refused:
                return False, f"the Portal logged '{REFUSED}': {refused[-1]}"
            meta = served()
            if marked(meta):
                return True, f"the Portal wrote {namespace}/{SECRET} back (resourceVersion {before} -> {meta['resourceVersion']})"
            # The second run started after the marker went, so its edge step saw it missing.
            if len(leader_runs(lines)) >= 2:
                return False, f"two Portal leader sync runs finished without writing {namespace}/{SECRET} back"
            if clock() >= deadline:
                return False, (f"the Portal did not write {namespace}/{SECRET} within {timeout:.0f} s ({basis}): "
                               "no sync ran, or its write never landed")
            sleep(poll)
    finally:
        restore(namespace)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    # Default: measured from the Portal's sync runs (measured_timeout).
    parser.add_argument("--timeout", type=float, default=None)
    args = parser.parse_args()
    # A stopped smoke still puts the marker back (wave's finally).
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(1))
    try:
        ok, said = wave(args.timeout)
    except (RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError, KeyError) as err:
        ok, said = False, str(err)
    print(f"  {'ok  ' if ok else 'FAIL'}  edge wave: {said}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
