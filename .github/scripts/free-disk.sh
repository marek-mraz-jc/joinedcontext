#!/bin/sh
# A hosted runner used to ship ~14 GB free; the Portal and jc test builds need more. Drop the SDKs
# no job here uses (as crates/portal's ci-full does before its --all-targets link). Deleting them
# takes 60-110 s, so by default it runs in the background while the job's first steps (toolchain,
# caches, the build's early crates) go on: the space is free long before a link needs it.
# `--wait` deletes in the foreground. With at least NEED GB free already (the runners of
# 2026-11 have 85 of 145 GB free) nothing is deleted: the background delete competed with the
# cache extraction and the build for the disk (run 42829330070).
#   sh free-disk.sh [--wait]
#   sh free-disk.sh --selftest
set -eu
NEED=40

# enough <available GB>: whether the disk already has room
enough() { [ "$1" -ge "$NEED" ]; }

if [ "${1:-}" = --selftest ]; then
  enough 85 && enough 40 && ! enough 39 && ! enough 14
  echo "free-disk.sh selftest ok"
  exit 0
fi
df -h / | tail -1
avail=$(df -BG --output=avail / | tail -1 | tr -dc 0-9)
if enough "$avail"; then
  echo "${avail} GB free, at least $NEED: nothing to delete"
  exit 0
fi
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
