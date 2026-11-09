#!/usr/bin/env bash
# jcs-of.sh <jc.toml>: the pid of every process started with `--config <jc.toml>`, one per line.
# up.sh stops each before it starts the next jc: a jc its pid file no longer names (an import's
# own restart, a hung one) kept running beside the new one on the same databases once.
set -u
want=${1:?jc.toml}
for d in /proc/[0-9]*; do
  [ "${d#/proc/}" = "$$" ] && continue
  # A process may exit between the glob and the read: the redirect then fails, silently.
  args=$( { tr '\0' '\n' < "$d/cmdline"; } 2>/dev/null) || continue
  if printf '%s\n' "$args" | grep -qxF -- "--config" && printf '%s\n' "$args" | grep -A1 -xF -- "--config" | tail -1 | grep -qxF -- "$want"; then
    echo "${d#/proc/}"
  fi
done
