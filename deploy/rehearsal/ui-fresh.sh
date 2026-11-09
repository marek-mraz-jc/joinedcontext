#!/usr/bin/env bash
# ui-fresh.sh <clone> <jc binary>: fails unless the Portal UI jc embeds (crates/portal/ui/dist,
# compiled in by rust-embed) was built after the last commit touching the UI or the SDK it imports,
# and the binary was linked after that build. A stale ui/dist ships an old Portal UI silently.
set -euo pipefail
clone=${1:?clone}; bin=${2:?jc binary}
dist=$clone/crates/portal/ui/dist/index.html
[ -f "$dist" ] || { echo "ui-fresh: no $dist: run pnpm build in crates/portal/ui first" >&2; exit 1; }
src=$(git -C "$clone" log -1 --format=%ct -- crates/portal/ui/src crates/portal/ui/index.html \
  crates/portal/ui/package.json crates/portal/ui/pnpm-lock.yaml crates/portal/sdk/src crates/portal/sdk/package.json)
built=$(stat -c %Y "$dist"); linked=$(stat -c %Y "$bin")
if [ "$built" -lt "${src:-0}" ]; then
  echo "ui-fresh: ui/dist built $(date -u -d "@$built" +%FT%TZ), before the last UI commit $(date -u -d "@$src" +%FT%TZ)" >&2
  exit 1
fi
if [ "$linked" -lt "$built" ]; then
  echo "ui-fresh: $bin linked $(date -u -d "@$linked" +%FT%TZ), before ui/dist $(date -u -d "@$built" +%FT%TZ)" >&2
  exit 1
fi
