#!/bin/sh
# Runs a memory-hungry command on a hosted runner (16 GB, sudo, cgroup v2) so that it cannot take
# the runner down: a starved runner loses contact with GitHub and the job's log is never written
# (T-7513: "The hosted runner lost communication with the server", portal-test-1).
# The command and its children run in a cgroup of MAX memory and SWAP swap: past that the kernel
# kills its largest process, the command fails and the log says so. Outside that cgroup, every
# TICK seconds one line says memory, swap, disk and the three largest processes (a rustc by its
# crate), so the log of a step ended by its timeout-minutes names what was running.
#   sh watched.sh CMD ARG...
#   sh watched.sh --selftest
set -eu
MAX=13G
SWAP=4G
TICK=20

# largest: the first three of `ps -eo rss=,comm=,args= --sort=-rss` on one line; of the arguments
# only a rustc's crate name and --test are printed (arguments may hold credentials)
largest() {
  awk 'NR <= 3 {
    name = $2
    if ($2 == "rustc") for (i = 3; i <= NF; i++) {
      if ($i == "--crate-name") name = name " " $(i + 1)
      if ($i == "--test") name = name " --test"
    }
    out = out (NR > 1 ? ", " : "") sprintf("%s %d MB", name, $1 / 1024)
  } END { print out }'
}

# oom_kills: the count in a memory.events file
oom_kills() { awk '$1 == "oom_kill" { print $2 }' "$1"; }

if [ "${1:-}" = --selftest ]; then
  got=$(printf '%s\n' '8388608 rustc rustc --crate-name portal src/lib.rs --test' \
    '2097152 rustc rustc --crate-name portal src/lib.rs' '1024 psql psql --password=hidden' '1 sh sh' | largest)
  [ "$got" = 'rustc portal --test 8192 MB, rustc portal 2048 MB, psql 1 MB' ] || { echo "largest: $got"; exit 1; }
  [ "$(printf 'oom 3\noom_kill 2\noom_group_kill 0\n' | oom_kills -)" = 2 ] || { echo "oom_kills"; exit 1; }
  echo "watched.sh selftest ok"
  exit 0
fi

cg=/sys/fs/cgroup/watched-$$
sudo mkdir "$cg"
echo "$MAX" | sudo tee "$cg/memory.max" >/dev/null
echo "$SWAP" | sudo tee "$cg/memory.swap.max" >/dev/null
(
  set +e
  while sleep "$TICK" >/dev/null 2>&1; do
    echo "$(date -u +%T) $(free -m | awk '/Mem/ {printf "mem used %s/%s MB", $3, $2} /Swap/ {printf " swap %s MB", $3}')" \
      "$(df -h / | awk 'NR == 2 {print "disk free " $4}') oom-killed $(oom_kills "$cg/memory.events")" \
      "largest: $(ps -eo rss=,comm=,args= --sort=-rss | largest)"
  done
) &
watch=$!
status=0
# shellcheck disable=SC2016 # $$ and $@ are the inner shell's
sh -c 'echo $$ | sudo tee "$0/cgroup.procs" >/dev/null && exec "$@"' "$cg" "$@" || status=$?
kill "$watch"
echo "watched.sh: exit $status, peak $(($(cat "$cg/memory.peak") / 1048576)) MB of $MAX with file cache," \
  "swap peak $(($(cat "$cg/memory.swap.peak") / 1048576)) MB of $SWAP"
killed=$(oom_kills "$cg/memory.events")
if [ "$killed" != 0 ]; then
  echo "::error::out of memory: the kernel killed $killed process(es) of '$1' at $MAX + $SWAP swap; the 'largest:' lines above name them"
fi
exit "$status"
