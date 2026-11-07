#!/bin/sh
# The forge's runner (ADR-N-028 §3.3, AP-81): one ephemeral registration per job, so a job never
# holds a runner credential a later job could use. In host mode a job runs as this same user
# and can read what this user reads (ADR-N-028 §5), so:
#
#   - the organization's registration token is read ONCE from a file the pod's init container
#     copied into memory, and the file is deleted before the first job starts; it lives on only
#     in this shell's unexported variable, which a job cannot read from /proc/<pid>/environ;
#   - it reaches `register` on stdin, never on a command line a job could list;
#   - the ephemeral registration file a job can read is spent by that job, and is deleted with
#     the work directory before the next registration;
#   - every process a job leaves behind is killed before the next registration, so a secret
#     given to a job that runs no application code is out of reach of the jobs before it.
#
#   GITEA_INSTANCE           the forge's in-cluster URL
#   GITEA_RUNNER_TOKEN_FILE  the copied registration token (default /tmp/runner-secret/token)
#   GITEA_RUNNER_NAME        a name prefix (default: the pod's hostname)
#   GITEA_RUNNER_ONCE        1 in an App's build pod (AP-130): register once with that App's
#                            repository token, take one job and end, so the Job completes
set -eu

fail() { echo "runner: $*" >&2; exit 1; }
: "${GITEA_INSTANCE:?GITEA_INSTANCE is not set}"
TOKEN_FILE=${GITEA_RUNNER_TOKEN_FILE:-/tmp/runner-secret/token}
[ -s "$TOKEN_FILE" ] || fail "no registration token at $TOKEN_FILE: it is read once per pod start, so delete the pod to restart the runner"
token=$(cat "$TOKEN_FILE")
rm -f "$TOKEN_FILE" || fail "cannot delete $TOKEN_FILE, and a job would read it"
[ ! -e "$TOKEN_FILE" ] || fail "$TOKEN_FILE is still there after rm, and a job would read it"

CONFIG=/opt/runner/runner.yaml
STATE=/tmp/runner
NAME=${GITEA_RUNNER_NAME:-$(hostname)}
job=0
while :; do
  job=$((job + 1))
  rm -rf "$STATE" && mkdir -p "$STATE"
  export HOME=$STATE
  cd "$STATE"
  if ! printf '%s' "$token" | gitea-runner -c "$CONFIG" register --ephemeral --no-interactive \
      --instance "$GITEA_INSTANCE" --token-file /dev/stdin --name "$NAME-$job"; then
    echo "runner: registration failed; trying again in 10 s" >&2
    sleep 10
    continue
  fi
  # Ephemeral: the daemon takes exactly one job and exits. A failed job is the forge's to report.
  gitea-runner -c "$CONFIG" daemon || echo "runner: the daemon ended with $?" >&2
  # A build pod holds one repository's token for one job; the next job gets a pod of its own.
  [ "${GITEA_RUNNER_ONCE:-}" = 1 ] && exit 0
  # Nothing a job started outlives it. A process an application step leaves behind would
  # otherwise read the environment of every later step and job on this pod, the lane's
  # secret included (/proc/<pid>/environ is readable by the same user). Everything in this
  # container runs as this user, so every process but this loop goes.
  for proc in /proc/[0-9]*; do
    pid=${proc#/proc/}
    [ "$pid" = "$$" ] || kill -9 "$pid" 2>/dev/null || true
  done
done
