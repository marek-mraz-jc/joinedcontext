#!/usr/bin/env bash
# stop-jc.sh <pid>: stops an earlier jc. SIGTERM, 10 s for its graceful shutdown, then SIGKILL:
# a jc whose runtime is deadlocked never finishes the graceful path and keeps its port, so the
# next jc would fail on "Address already in use". Fails if the process outlives SIGKILL.
pid=${1:?pid}
# alive: exists and is no zombie (a killed child its parent has not reaped yet holds no port)
alive() { kill -0 "$pid" 2>/dev/null && [ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null)" != Z ]; }
kill "$pid" 2>/dev/null || exit 0
for _ in $(seq 20); do alive || exit 0; sleep 0.5; done
echo "stop-jc: $pid ignored SIGTERM for 10 s; SIGKILL" >&2
kill -9 "$pid" 2>/dev/null || exit 0
for _ in $(seq 10); do alive || exit 0; sleep 0.5; done
echo "stop-jc: $pid still runs after SIGKILL" >&2
exit 1
