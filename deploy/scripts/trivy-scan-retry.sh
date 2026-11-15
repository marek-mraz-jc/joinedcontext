#!/usr/bin/env bash
#
# Scan one pinned image with trivy, retrying only a registry rate limit (T-4704).
#
#   scripts/trivy-scan-retry.sh <image@sha256:...> <report.json>
#
# A TOOMANYREQUESTS / 429 answer is retried after 30 s, 90 s and 270 s. ECR Public refuses shared
# runners for many minutes (runs 39173636688, 39180110213, 39237147977), and its docker/library is
# Docker's official images, so a retry of such an image reads the same digest from mirror.gcr.io and
# the report keeps the pinned name: the same bytes are scanned and no image is skipped. Any other
# trivy error fails at once. An image still refused after the last retry fails with
# "registry rate limit on <image>, not scanned": an unscanned pin is never a pass.
# TRIVY and SLEEP override the commands for scripts/trivy-scan-retry.test.sh.
set -euo pipefail

image=${1:?usage: trivy-scan-retry.sh <image> <report.json>}
out=${2:?usage: trivy-scan-retry.sh <image> <report.json>}
trivy=${TRIVY:-trivy}
pause=${SLEEP:-sleep}
log=$(mktemp)
trap 'rm -f "$log"' EXIT

src=$image
for wait in 30 90 270 last; do
  # stderr goes to a file, then to the log: a `2> >(tee ...)` raced the grep below on the runner.
  rc=0
  "$trivy" image --quiet --scanners vuln --severity CRITICAL,HIGH --format json --output "$out" "$src" 2> "$log" || rc=$?
  cat "$log" >&2
  if [ "$rc" -eq 0 ]; then
    if [ "$src" != "$image" ]; then
      jq --arg name "$image" '.ArtifactName = $name' "$out" > "$out.tmp" && mv "$out.tmp" "$out"
    fi
    exit 0
  fi
  if ! grep -Eqi 'TOOMANYREQUESTS|429 Too Many Requests|status(code)?[: ]+429' "$log"; then
    echo "trivy failed on $src (not a rate limit), not scanned" >&2
    exit 1
  fi
  [ "$wait" = last ] && break
  echo "registry rate limit on $src, retrying in ${wait}s" >&2
  "$pause" "$wait"
  case $image in
    ecr-public.aws.com/docker/library/*) src="mirror.gcr.io/library/${image#ecr-public.aws.com/docker/library/}" ;;
  esac
done
echo "registry rate limit on $image, not scanned" >&2
exit 1
