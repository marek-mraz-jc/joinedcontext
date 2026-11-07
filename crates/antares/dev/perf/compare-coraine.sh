#!/usr/bin/env bash
# Antares and Coraine on the same machine, measured the same way (T-3548,
# docs/analysis/coraine-comparison.md): the files each process maps, idle
# RSS, start to first answer, RSS with 100 000 entities, and the T-3467
# request shapes (dev/perf/k6-shapes.js speaks plain NGSI-LD, so both
# brokers get the identical load). Each broker is pinned to CPU 0 and k6
# runs on the others.
#
#   BIN=target/release/antares CORAINE_IMAGE=quay.io/seamware/coraine:0.5.0 \
#     OUT=results/coraine dev/perf/compare-coraine.sh
#
# Runs on a GitHub-hosted runner (compare-coraine.yml), never on a shared
# host. Coraine runs from its published image with --net=host, which its
# README measures as no different from native. A container's process
# belongs to root, so its /proc entries are read through sudo.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN="${BIN:-target/release/antares}"
CORAINE_IMAGE="${CORAINE_IMAGE:?name the coraine image tag}"
OUT="${OUT:-results/coraine}"
PORT=9473
RUNS="${RUNS:-3}"
DURATION="${DURATION:-5s}"
mkdir -p "$OUT"
command -v k6 >/dev/null || { echo "k6 missing"; exit 1; }
[ -x "$BIN" ] || { echo "missing $BIN"; exit 1; }
URL="http://127.0.0.1:$PORT"
LOAD_CPUS="1-$(( $(nproc) - 1 ))"
PID=
cleanup() {
  if [ -n "$PID" ]; then kill "$PID" 2>/dev/null || true; fi
  docker rm -f coraine-cmp >/dev/null 2>&1 || true
}
trap cleanup EXIT

rd() { cat "$1" 2>/dev/null || sudo cat "$1"; }
rss_kib() { rd "/proc/$PID/status" | awk '/^VmRSS/ {print $2}'; }

start() {  # start <broker>: sets PID to the broker's own process
  case $1 in
    antares)
      # no history on either side: Coraine runs with --troe none below
      ANTARES_STORE=memory ANTARES_TEMPORAL=none ANTARES_HTTP_PORT=$PORT ANTARES_ALLOW_SHARED_LOCAL=1 \
        taskset -c 0 "$BIN" > "$OUT/antares.log" 2>&1 &
      PID=$!
      ;;
    coraine)
      docker run -d --name coraine-cmp --net=host --cpuset-cpus=0 "$CORAINE_IMAGE" \
        --port "$PORT" --database corDB --troe none > /dev/null
      # the broker itself, not an entrypoint shell in front of it
      PID=
      for _ in $(seq 200); do
        PID=$(pgrep -x coraine || true)
        [ -n "$PID" ] && break
        sleep 0.05
      done
      [ -n "$PID" ] || PID=$(docker inspect -f '{{.State.Pid}}' coraine-cmp)
      ;;
  esac
}

stop() {
  case $1 in
    antares) kill "$PID"; wait "$PID" 2>/dev/null || true ;;
    coraine)
      docker logs coraine-cmp > "$OUT/coraine.log" 2>&1
      docker rm -f coraine-cmp > /dev/null
      ;;
  esac
  PID=
}

# Start to first answer: the process's own start time (/proc/<pid>/stat
# field 22, clock ticks since boot) against the boot clock when the first
# GET answers 200. The same method for both, so the image's container set-up
# is not counted against Coraine; resolution is one clock tick.
read -r -d '' FIRST_ANSWER <<'EOF' || true
import os, sys, time, urllib.request
url = sys.argv[1]
tick = os.sysconf("SC_CLK_TCK")
start = int(sys.stdin.read().rsplit(")", 1)[1].split()[19]) / tick
deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    try:
        if urllib.request.urlopen(url, timeout=1).status == 200:
            break
    except Exception:
        time.sleep(0.0005)
up = float(open("/proc/uptime").read().split()[0])
print(f"{(up - start) * 1000:.0f}")
EOF

first_answer_ms() {
  rd "/proc/$PID/stat" | python3 -c "$FIRST_ANSWER" "$URL/ngsi-ld/v1/entities?type=Probe"
}

# Every file the running broker maps (binary, its plugins, its libraries),
# each counted once, with its size: what has to be on the machine for it.
mapped() {
  local root=""
  [ "$1" = coraine ] && root="/proc/$PID/root"
  rd "/proc/$PID/maps" | awk '$6 ~ /^\// {print $6}' | sort -u | while read -r f; do
    printf '%s\t%s\n' "$(sudo stat -c %s "$root$f" 2>/dev/null || echo 0)" "$f"
  done > "$OUT/$1-maps.tsv"
  awk -F'\t' '{s += $1} END {printf "%.1f", s / 1048576}' "$OUT/$1-maps.tsv"
}

shape() {  # shape <name> <vus> <entities>: "<req/s> <p99 ms>", median of RUNS by req/s
  local rows=()
  for _ in $(seq "$RUNS"); do
    taskset -c "$LOAD_CPUS" k6 run --quiet --summary-export "$OUT/shape.json" \
      -e BROKER_URL="$URL" -e SHAPE="$1" -e VUS="$2" -e DURATION="$DURATION" \
      -e ENTITIES="$3" dev/perf/k6-shapes.js > /dev/null
    rows+=("$(python3 -c "
import json; s=json.load(open('$OUT/shape.json'))['metrics']
print(f\"{s['http_reqs']['rate']:.0f} {s['http_req_duration']['p(99)']:.2f}\")")")
  done
  printf '%s\n' "${rows[@]}" | sort -n | sed -n "$(( (RUNS + 1) / 2 ))p"
}

{
  echo "| broker | mapped files MiB | binary MiB | idle RSS MiB | first answer ms | RSS at 100k MiB |"
  echo "|---|---|---|---|---|---|"
} > "$OUT/footprint.md"
{
  echo "| broker | entities | shape | concurrency | req/s | p99 |"
  echo "|---|---|---|---|---|---|"
} > "$OUT/shapes.md"

for broker in antares coraine; do
  start "$broker"
  ms=$(first_answer_ms)
  rd "/proc/$PID/status" > /dev/null || { echo "$broker exited:"; tail -5 "$OUT/$broker.log" 2>/dev/null; exit 1; }
  sleep 2
  idle=$(rss_kib)
  maps=$(mapped "$broker")
  bin=$(awk -F'\t' '$2 ~ /\/(antares|coraine)$/ {printf "%.1f", $1 / 1048576; exit}' \
    "$OUT/$broker-maps.tsv")
  full=
  for spec in "1000 query 50" "1000 query-q 50" "1000 retrieve 50" "1000 upsert 50" \
              "1000 subscribe 50" "100000 query 50" "100000 query-q 50" "100000 retrieve 50"; do
    read -r n name vus <<<"$spec"
    if [ "$n" = 100000 ] && [ -z "$full" ]; then
      # each shape run deletes its entities in teardown: seed once without
      # it, so the RSS is read with the 100 000 entities held
      taskset -c "$LOAD_CPUS" k6 run --quiet --no-teardown -e BROKER_URL="$URL" \
        -e SHAPE=retrieve -e VUS=1 -e DURATION=1s -e ENTITIES="$n" dev/perf/k6-shapes.js > /dev/null
      full=$(rss_kib)
    fi
    echo "$broker $n $name" > "$OUT/phase"
    read -r rps p99 < <(shape "$name" "$vus" "$n")
    echo "| $broker | $n | $name | c$vus | $rps | $p99 ms |" >> "$OUT/shapes.md"
  done
  echo "| $broker | $maps | $bin | $(( idle / 1024 )) | $ms | $(( full / 1024 )) |" >> "$OUT/footprint.md"
  stop "$broker"
done
cat "$OUT/footprint.md" "$OUT/shapes.md"
