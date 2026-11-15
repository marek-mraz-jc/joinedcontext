#!/usr/bin/env bash
# T-5082: the catalogue explorer's table, the Portal's Endpoint data view, from one Portal commit.
#   scripts/viz-explorer-bundle.sh <portal clone> <portal sha>
# Takes ui/ and sdk/ at <sha> (git archive, not the clone's working tree), installs their locked
# dependencies, runs ui's vite.explorer.config.ts and writes explorer.js and explorer.css into
# images/viz/explorer with SHA256SUMS, whose first line names the commit. The image build checks the
# sums (images/viz/Dockerfile) and tests/test_viz.py checks that explore.html loads these names.
set -euo pipefail
portal="${1:?usage: viz-explorer-bundle.sh <portal clone> <portal sha>}"
sha="${2:?usage: viz-explorer-bundle.sh <portal clone> <portal sha>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/images/viz/explorer"
sha="$(git -C "$portal" rev-parse --verify "$sha^{commit}")"
work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT
git -C "$portal" archive "$sha" sdk ui | tar -x -C "$work"
(cd "$work/sdk" && pnpm install --frozen-lockfile --silent)
(cd "$work/ui" && pnpm install --frozen-lockfile --silent && npx --no-install vite build --config vite.explorer.config.ts >/dev/null)
mkdir -p "$out"
cp "$work/ui/dist-explorer/explorer.js" "$work/ui/dist-explorer/explorer.css" "$out/"
{ echo "# portal: $sha"; (cd "$out" && sha256sum explorer.css explorer.js); } > "$out/SHA256SUMS"
