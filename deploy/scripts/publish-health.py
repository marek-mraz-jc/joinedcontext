#!/usr/bin/env python3
"""Publish one validation check's summary to the Portal's health page (T-2803, OPS-53).

Reads the summary a check wrote for `tasks/file-failures` and writes its digest, the key
`{check}.json` of the ConfigMap `jc-validation-results` in the Portal's namespace, which the
Portal mounts at `JC_HEALTH_DIR` (API/01 §26):

- the verdict counts, and at most 50 failing or erroring results with key, verdict and title,
  each with the open task whose `check: {check}/{key}` line names it (or the check's overflow
  task);
- the history of the last seven days, at most 200 points, carried over from the digest already
  published;
- with `--passed`, the passing keys too (at most 500), which the App probe's chips need
  (AP-136).

A result's `detail` and `evidence` never leave the summary, and every text is redacted the way
`tasks/file-failures` redacts, so the page holds no secret. Run it after `tasks/file-failures`,
so the tasks it links exist.
"""

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

CONFIGMAP = "jc-validation-results"
PORTAL_LABEL = "app.kubernetes.io/name=portal-portal"
NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,39}$")
TASK_ID = re.compile(r"^id:\s*(T-\d{4,6})\s*$", re.M)
MAX_FAILURES, MAX_HISTORY, MAX_TEXT, MAX_PASSED, KEEP = 50, 200, 300, 500, timedelta(days=7)
# The patterns of tasks/file-failures: a token after its scheme, a JWT, a secret-named value,
# credentials in a URL, and any long base64-like run.
SECRETS = [
    (re.compile(r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=-]{8,}"), r"\1 [redacted]"),
    (re.compile(r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*"), "[redacted jwt]"),
    (re.compile(r"(?i)(password|passwd|secret|client_secret|api[_-]?key|access_token|refresh_token)(\"?\s*[:=]\s*\"?)[^\s\"&,;]+"),
     r"\1\2[redacted]"),
    (re.compile(r"(https?://)[^/\s:@]+:[^/\s@]+@"), r"\1[redacted]@"),
    (re.compile(r"\b[A-Za-z0-9+/_-]{40,}={0,2}"), "[redacted]"),
]


def text(value: object) -> str:
    out = " ".join(str(value or "").split())
    for pattern, replacement in SECRETS:
        out = pattern.sub(replacement, out)
    return out if len(out) <= MAX_TEXT else out[: MAX_TEXT - 1] + "…"


def open_tasks(board: Path | None, check: str) -> dict[str, str]:
    """`{key: task id}` for every open task naming `check: {check}/{key}`; `*overflow*` included."""
    if board is None or not board.is_dir():
        return {}
    line = re.compile(rf"^check:\s*{re.escape(check)}/(.+?)\s*$", re.M)
    tasks = {}
    for path in sorted(board.glob("T-*.md")):
        body = path.read_text(errors="replace")
        found = TASK_ID.search(body)
        if found:
            for key in line.findall(body):
                tasks.setdefault(key, found.group(1))
    return tasks


def recent(point: object, now: datetime) -> bool:
    """A point of the published history that is well formed and at most seven days old."""
    try:
        at = datetime.fromisoformat(point["at"].replace("Z", "+00:00"))
    except (TypeError, KeyError, AttributeError, ValueError):
        return False
    return now - at <= KEEP and all(isinstance(point.get(v), int) for v in ("pass", "fail", "error", "skip"))


def digest(summary: dict, every_hours: int, now: datetime, previous: dict | None, tasks: dict[str, str],
           passed: bool = False) -> dict:
    check = summary.get("check", "")
    if not NAME.match(check):
        raise SystemExit(f"publish-health: the summary names no check, or '{check}' is not a check name")
    results = summary.get("results")
    if not isinstance(results, list):
        raise SystemExit("publish-health: the summary has no results list")
    counts = {verdict: 0 for verdict in ("pass", "fail", "error", "skip")}
    failures, passes = [], []
    for item in results:
        verdict = item.get("verdict")
        if verdict not in counts:
            raise SystemExit(f"publish-health: result {item.get('key')!r} has verdict {verdict!r}")
        counts[verdict] += 1
        if verdict == "pass" and passed:
            passes.append(text(item.get("key", "")))
        if verdict in ("fail", "error") and len(failures) < MAX_FAILURES:
            key = str(item.get("key", ""))
            failure = {"key": text(key), "verdict": verdict, "title": text(item.get("title"))}
            task = tasks.get(key) or tasks.get("*overflow*")
            if task:
                failure["task"] = task
            failures.append(failure)
    at = now.strftime("%Y-%m-%dT%H:%M:%SZ")
    history = [p for p in (previous or {}).get("history", []) if recent(p, now)]
    history = (history + [{"at": at, **counts}])[-MAX_HISTORY:]
    out = {"check": check, "at": at, "everyHours": every_hours, "counts": counts,
           "failures": failures, "history": history}
    if summary.get("run"):
        out["run"] = text(summary["run"])
    if passed:
        # The Portal shows a chip per passing key (the App probe, AP-136) and refuses a digest
        # with more than it can show rather than a partial list.
        if len(passes) > MAX_PASSED:
            raise SystemExit(f"publish-health: {len(passes)} passing keys, the Portal shows at most {MAX_PASSED}")
        out["passed"] = passes
    return out


def kubectl(*args: str) -> str:
    return subprocess.run(["kubectl", *args], check=True, capture_output=True, text=True, timeout=60).stdout


def portal_namespace() -> str:
    namespace = kubectl("get", "pods", "-A", "-l", PORTAL_LABEL, "-o", "jsonpath={.items[0].metadata.namespace}").strip()
    if not namespace:
        raise SystemExit(f"publish-health: no Portal pod ({PORTAL_LABEL}) in the cluster: name it with --namespace")
    return namespace


def published(namespace: str, check: str) -> dict | None:
    """The digest already published for `check`, or None; an unreadable one starts a new history."""
    try:
        raw = kubectl("get", "configmap", CONFIGMAP, "-n", namespace, "-o", "json")
    except subprocess.CalledProcessError as err:
        if "NotFound" in err.stderr:
            return None
        raise
    try:
        return json.loads(json.loads(raw).get("data", {}).get(f"{check}.json", "null"))
    except (json.JSONDecodeError, TypeError):
        return None


def publish(namespace: str, check: str, body: dict) -> None:
    patch = {
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": CONFIGMAP, "namespace": namespace,
                     "labels": {"app.kubernetes.io/part-of": "joinedcontext"}},
        "data": {f"{check}.json": json.dumps(body, ensure_ascii=False)},
    }
    # Server-side apply with one field manager per check: each check owns its own key, so two
    # checks publishing at once never overwrite each other.
    subprocess.run(["kubectl", "apply", "--server-side", f"--field-manager=publish-health-{check}", "-f", "-"],
                   input=json.dumps(patch), check=True, capture_output=True, text=True, timeout=60)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("summary", type=Path, help="the summary JSON the check wrote")
    parser.add_argument("--every-hours", type=int, required=True, help="how often the check runs, 1 to 744 (a month)")
    parser.add_argument("--tasks-dir", type=Path, default=Path("/workspace/tasks"), help="the board, for the task ids")
    parser.add_argument("--namespace", help="the Portal's namespace; found by its pod label when left out")
    parser.add_argument("--passed", action="store_true",
                        help="also list the passing keys, for a check whose passes a page shows (apps)")
    parser.add_argument("--dry-run", action="store_true", help="print the digest, publish nothing")
    args = parser.parse_args(argv)
    if not 1 <= args.every_hours <= 744:
        raise SystemExit("publish-health: --every-hours is 1 to 744")
    summary = json.loads(args.summary.read_text())
    check = summary.get("check", "")
    now = datetime.now(timezone.utc).replace(microsecond=0)
    namespace = None if args.dry_run else (args.namespace or portal_namespace())
    previous = published(namespace, check) if namespace else None
    body = digest(summary, args.every_hours, now, previous, open_tasks(args.tasks_dir, check), args.passed)
    if args.dry_run:
        print(json.dumps(body, indent=2, ensure_ascii=False))
        return 0
    publish(namespace, check, body)
    print(f"published {check}: {body['counts']} to {namespace}/{CONFIGMAP}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
