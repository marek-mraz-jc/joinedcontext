#!/usr/bin/env bash
# wait-jc.sh <pid> <log> [timeout s]: up.sh's start check. Fails (log tail on stderr) when jc
# exits before it prints "jc listening", or within 10 s after (a config the binary refuses, a
# module that dies at boot); answers 0 when it listens and stays up.
set -euo pipefail
pid=${1:?pid}; log=${2:?log}; timeout=${3:-180}; settle=${REHEARSAL_SETTLE_SECS:-10}
fail() { echo "jc $1" >&2; tail -40 "$log" >&2; exit 1; }
# An exited jc its parent has not reaped yet is a zombie: kill -0 still finds it.
alive() { kill -0 "$pid" 2>/dev/null && [ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null)" != Z ]; }
for _ in $(seq "$timeout"); do
  grep -q "jc listening" "$log" && break
  alive || fail "exited before it listened"
  sleep 1
done
grep -q "jc listening" "$log" || fail "did not listen within ${timeout} s"
for _ in $(seq "$settle"); do
  alive || fail "exited within ${settle} s of listening"
  sleep 1
done
