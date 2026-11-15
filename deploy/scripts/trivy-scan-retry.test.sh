#!/usr/bin/env bash
#
# Test for scripts/trivy-scan-retry.sh (T-4704) with a fake trivy: two rate limits then a pass,
# a rate limit that never lifts, a non-rate-limit error, and the mirror copy keeping the pinned name.
#
#   scripts/trivy-scan-retry.test.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT
fail=0

# The fake trivy fails with $FAKE_ERROR for its first $FAKE_FAILS calls, then writes a report.
cat > "$tmp/trivy" <<'FAKE'
#!/usr/bin/env bash
n=$(( $(cat "$FAKE_DIR/calls" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$FAKE_DIR/calls"
echo "${*: -1}" >> "$FAKE_DIR/sources"
if [ "$n" -le "$FAKE_FAILS" ]; then echo "$FAKE_ERROR" >&2; exit 1; fi
out=; while [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done
printf '{"ArtifactName":"%s","Results":[]}\n' "$(tail -1 "$FAKE_DIR/sources")" > "$out"
FAKE
chmod +x "$tmp/trivy"
# shellcheck disable=SC2016 # the fake's own $1, expanded when it runs
printf '#!/usr/bin/env bash\necho "$1" >> "$FAKE_DIR/sleeps"\n' > "$tmp/sleep"
chmod +x "$tmp/sleep"

# case <name> <fails> <error> <image> -> sets rc, err, calls, sleeps, sources
run() {
  local d="$tmp/$1"; mkdir -p "$d"
  set +e
  FAKE_DIR=$d FAKE_FAILS=$2 FAKE_ERROR=$3 TRIVY="$tmp/trivy" SLEEP="$tmp/sleep" \
    "$here/trivy-scan-retry.sh" "$4" "$d/report.json" 2> "$d/err"
  rc=$?
  set -e
  err=$(cat "$d/err"); calls=$(cat "$d/calls"); sleeps=$(paste -sd' ' "$d/sleeps" 2>/dev/null || true)
  sources=$(paste -sd' ' "$d/sources"); report="$d/report.json"
}
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fail=1; fi; }

limit='GET https://ecr-public.aws.com/v2/...: TOOMANYREQUESTS: Data limit exceeded'
img='ghcr.io/example/app@sha256:aaaa'

run twice-then-pass 2 "$limit" "$img"
check "two rate limits then a pass: exit 0" "$rc" 0
check "two rate limits then a pass: three calls" "$calls" 3
check "two rate limits then a pass: backoff 30 90" "$sleeps" "30 90"

run never-lifts 99 "$limit" "$img"
check "rate limit never lifts: exit 1" "$rc" 1
check "rate limit never lifts: four calls" "$calls" 4
check "rate limit never lifts: backoff 30 90 270" "$sleeps" "30 90 270"
check "rate limit never lifts: message" "$(grep -c "registry rate limit on $img, not scanned" <<<"$err")" 1

run cve-or-other-error 99 'FATAL: unable to parse image manifest' "$img"
check "other error: exit 1" "$rc" 1
check "other error: no retry" "$calls" 1
check "other error: no sleep" "$sleeps" ""
check "other error: says not scanned" "$(grep -c "not a rate limit), not scanned" <<<"$err")" 1

run http-429 1 'unexpected status code 429 Too Many Requests' "$img"
check "429: retried then pass" "$rc/$calls" 0/2

ecr='ecr-public.aws.com/docker/library/redis@sha256:bbbb'
run ecr-mirror 1 "$limit" "$ecr"
check "ecr rate limit: pass from the mirror" "$rc" 0
check "ecr rate limit: second call reads mirror.gcr.io" "$sources" "$ecr mirror.gcr.io/library/redis@sha256:bbbb"
check "ecr rate limit: report keeps the pinned name" "$(jq -r .ArtifactName "$report")" "$ecr"

exit "$fail"
