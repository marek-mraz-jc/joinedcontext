#!/usr/bin/env bash
# T-4694: the standalone assistant's bundle (components/ai) from one platform commit.
#   scripts/ai-bundle.sh <platform clone> <platform sha> [out dir]
# Takes the widget sources at <sha> (git archive, not the clone's working tree), runs their
# build.mjs --template into [out dir] (default: the chart's files/site) and writes
# components/ai/bundle.lock: the sha and each file's sha256. tests/test_ai_app.py rebuilds from
# the lock and compares, so a hand edit or a stale bundle fails there.
set -euo pipefail
platform="${1:?usage: ai-bundle.sh <platform clone> <platform sha> [out dir]}"
sha="${2:?usage: ai-bundle.sh <platform clone> <platform sha> [out dir]}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${3:-$root/components/ai/charts/app/files/site}"
sha="$(git -C "$platform" rev-parse --verify "$sha^{commit}")"
work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT
git -C "$platform" archive "$sha" crates/assistant/widget crates/jc-core/data/field-order.json | tar -x -C "$work"
mkdir -p "$out"
node "$work/crates/assistant/widget/app/build.mjs" --template --out "$out" >/dev/null
if [ "$#" -lt 3 ]; then
  { echo "platform: $sha"; (cd "$out" && sha256sum -- * | sort -k2); } > "$root/components/ai/bundle.lock"
fi
