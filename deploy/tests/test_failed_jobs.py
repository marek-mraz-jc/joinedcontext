"""scripts/failed-jobs.py, the dev-smoke check of CronJobs whose newest Job failed (T-4385)."""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/failed-jobs.py"
spec = importlib.util.spec_from_file_location("failed_jobs", SCRIPT)
failed_jobs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(failed_jobs)


def job(name, cronjob, created, **status):
    owners = [{"kind": "CronJob", "name": cronjob}] if cronjob else []
    if status.pop("retrying", False):
        pass
    elif status.get("succeeded"):
        status["conditions"] = [{"type": "Complete", "status": "True"}]
    elif status.get("failed"):
        status["conditions"] = [{"type": "Failed", "status": "True", "reason": "BackoffLimitExceeded"}]
    return {"metadata": {"name": name, "creationTimestamp": created, "ownerReferences": owners}, "status": status}


def test_only_the_newest_finished_job_of_each_cronjob_counts():
    jobs = {"items": [
        job("ckan-harvest-bbsk-1", "ckan-harvest-bbsk", "2026-10-16T03:23:00Z", failed=2),
        job("ckan-harvest-bbsk-2", "ckan-harvest-bbsk", "2026-10-17T03:23:00Z", succeeded=1),
        job("ckan-harvest-banskabystrica-2", "ckan-harvest-banskabystrica", "2026-10-17T03:23:00Z", failed=4),
        job("ckan-harvest-banskabystrica-1", "ckan-harvest-banskabystrica", "2026-10-16T03:23:00Z", succeeded=1),
        job("ckan-harvest-zilina-3", "ckan-harvest-zilina", "2026-10-17T03:23:00Z", active=1),
        job("ckan-harvest-zilina-2", "ckan-harvest-zilina", "2026-10-16T03:23:00Z", succeeded=1),
        job("api-token-now", None, "2026-10-17T03:00:00Z", failed=1),
    ]}
    assert failed_jobs.failed(jobs) == [
        ("ckan-harvest-banskabystrica", "ckan-harvest-banskabystrica-2", "BackoffLimitExceeded")]


def test_the_script_fails_only_on_a_failed_cronjob():
    def run(items):
        return subprocess.run([sys.executable, str(SCRIPT)], input=json.dumps({"items": items}),
                              capture_output=True, text=True)
    red = run([job("h-1", "h", "2026-10-17T03:23:00Z", failed=1)])
    assert red.returncode == 1 and "CronJob h: its newest Job h-1 failed" in red.stdout
    green = run([])
    assert green.returncode == 0 and green.stdout == ""


def test_a_job_retrying_under_its_backoff_limit_is_not_failed_yet():
    jobs = {"items": [
        job("h-2", "h", "2026-10-17T03:23:00Z", active=1, failed=1, retrying=True),
        job("h-1", "h", "2026-10-16T03:23:00Z", succeeded=1),
    ]}
    assert failed_jobs.failed(jobs) == []
