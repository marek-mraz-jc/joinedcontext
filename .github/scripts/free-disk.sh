#!/bin/sh
# A hosted runner ships ~14 GB free; the Portal and jc test builds need more. Drop the SDKs no job
# here uses (as crates/portal's ci-full does before its --all-targets link). Deleting them takes
# 60-110 s, so by default it runs in the background while the job's first steps (toolchain,
# caches, the build's early crates) go on: the space is free long before a link needs it.
# `--wait` deletes in the foreground.
set -eu
df -h / | tail -1
dirs="/usr/share/dotnet /usr/local/lib/android /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL"
if [ "${1:-}" = --wait ]; then
  # shellcheck disable=SC2086
  sudo rm -rf $dirs
  df -h / | tail -1
else
  # shellcheck disable=SC2086
  nohup sudo rm -rf $dirs >/dev/null 2>&1 &
  echo "freeing disk in the background (pid $!)"
fi
