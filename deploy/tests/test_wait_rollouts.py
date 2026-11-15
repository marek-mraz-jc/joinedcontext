"""scripts/wait-rollouts.sh against a stub kubectl: a pod that is not ready fails the apply, the old
pod of a finished rollout does not, even once its containers exited and it shows Error, and
neither does one attempt of a Job that has not failed, nor a failed run of a CronJob whose newer
run completed."""

import os
import subprocess
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "wait-rollouts.sh"

KUBECTL = """#!/usr/bin/env bash
[ -n "$CALLS" ] && echo "$*" >>"$CALLS"
case "$*" in
  "get ns -o name") printf '%s\\n' ${NAMESPACES:-namespace/dev} ;;
  "get pod "*) [ "$LIVE_OWNER" = gone ] && exit 1; printf '%s' "$LIVE_OWNER" ;;
  *deletionTimestamp*) printf '%s' "$DELETING" ;;
  "get jobs"*) printf '%s' "$JOBS" ;;
  *ownerReferences*) printf 'old-7f5c \\nnew-8dc8 \\nlane-29840325-mfgw4 lane-29840325\\n' ;;
  *"--no-headers"*) printf 'old-7f5c 0/2 Error 0 113m\\nnew-8dc8 2/2 Running 0 3m\\njob-x 0/1 Completed 0 1h\\nlane-29840325-mfgw4 0/2 Error 0 4m\\n%s' "$LATE" ;;
  *) ;;
esac
"""


def run(tmp_path: Path, deleting: str, jobs: str = "lane-29840325|||lane|2026-10-24T08:00:00Z\n", late: str = "", live_owner: str = "", namespaces: str = "") -> subprocess.CompletedProcess:
    stub = tmp_path / "kubectl"
    stub.write_text(KUBECTL)
    stub.chmod(0o755)
    env = dict(os.environ, PATH=f"{tmp_path}:{os.environ['PATH']}", DELETING=deleting, JOBS=jobs, LATE=late, LIVE_OWNER=live_owner,
               NAMESPACES=namespaces, CALLS=str(tmp_path / "calls.log"))
    return subprocess.run(["bash", str(SCRIPT), "dev", "5"], env=env, capture_output=True, text=True)


def test_a_pod_being_deleted_is_not_a_failed_rollout(tmp_path):
    done = run(tmp_path, "old-7f5c\n")
    assert done.returncode == 0, done.stdout
    assert "not ready" not in done.stdout


def test_a_pod_in_error_that_nobody_deletes_fails_the_apply(tmp_path):
    done = run(tmp_path, "")
    assert done.returncode == 1
    assert "--- not ready: dev/old-7f5c" in done.stdout
    assert "new-8dc8" not in done.stdout.split("--- not ready")[1]


def test_an_attempt_of_a_job_still_retrying_is_not_a_failure(tmp_path):
    done = run(tmp_path, "old-7f5c\n")
    assert done.returncode == 0, done.stdout
    assert "lane-29840325-mfgw4" not in done.stdout


def test_the_pod_of_a_failed_job_fails_the_apply(tmp_path):
    done = run(tmp_path, "old-7f5c\n", jobs=FAILED)
    assert done.returncode == 1
    assert "--- not ready: dev/lane-29840325-mfgw4" in done.stdout


FAILED = "lane-29840325|True||lane|2026-10-24T08:00:00Z\n"


def test_a_failed_cronjob_run_a_newer_run_completed_is_not_a_failure(tmp_path):
    # One run failed while the upgrade restarted Gitea, the next one completed (ci-full 40074726891).
    done = run(tmp_path, "old-7f5c\n", jobs=FAILED + "lane-29840330||True|lane|2026-10-24T08:05:00Z\n")
    assert done.returncode == 0, done.stdout


def test_a_failed_cronjob_run_newer_than_its_last_success_fails_the_apply(tmp_path):
    done = run(tmp_path, "old-7f5c\n", jobs="lane-29840320||True|lane|2026-10-24T07:55:00Z\n" + FAILED)
    assert done.returncode == 1
    assert "--- not ready: dev/lane-29840325-mfgw4" in done.stdout


def test_a_success_of_another_cronjob_does_not_excuse_a_failed_run(tmp_path):
    done = run(tmp_path, "old-7f5c\n", jobs=FAILED + "seed-29840330||True|seed|2026-10-24T08:05:00Z\n")
    assert done.returncode == 1


LATE = "token-29861355-w8scj 0/2 ContainerCreating 0 3s\n"


def test_a_cronjob_pod_started_after_the_snapshot_is_not_a_failure(tmp_path):
    # The pod is in the final listing but not in the owners read before it (T-3537 dev smoke).
    done = run(tmp_path, "old-7f5c\n", late=LATE, live_owner="token-29861355")
    assert done.returncode == 0, done.stdout


def test_a_late_pod_gone_by_the_time_it_is_read_is_not_a_failure(tmp_path):
    done = run(tmp_path, "old-7f5c\n", late=LATE, live_owner="gone")
    assert done.returncode == 0, done.stdout


def test_a_late_pod_that_no_job_owns_still_fails_the_apply(tmp_path):
    done = run(tmp_path, "old-7f5c\n", late=LATE, live_owner="")
    assert done.returncode == 1
    assert "--- not ready: dev/token-29861355-w8scj" in done.stdout


def test_only_the_instance_s_own_namespaces_are_waited_on(tmp_path):
    # F-R2DEP3-017: `^(dev|dev-|...)` without an end anchor took `development` and `devops-x`.
    names = "namespace/dev namespace/dev-helsinki-apps namespace/development namespace/devops-x namespace/jc-operators"
    run(tmp_path, deleting="old-7f5c", namespaces=names)
    asked = (tmp_path / "calls.log").read_text()
    assert "-n dev-helsinki-apps " in asked and "-n jc-operators " in asked, asked
    assert "development" not in asked.replace("get ns -o name", "") and "devops-x" not in asked
