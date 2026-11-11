#!/bin/sh
# The open-data explorer bundle (vendor/viz/explorer) built from this tree's Portal UI (T-6739):
# the Portal and open data draw one grid, so the committed bundle must be the one these sources
# build. Run from crates/portal/ui after `pnpm install` (and the SDK's): no argument checks that
# the build matches the committed files and both SHA256SUMS; `--write` puts the build in place.
set -eu
vendor=../../../vendor/viz
[ -f vite.explorer.config.ts ] || { echo "explorer-bundle: run from crates/portal/ui" >&2; exit 2; }
pnpm exec vite build -c vite.explorer.config.ts >/dev/null
sum() { sha256sum "$1" | cut -d' ' -f1; }
if [ "${1:-}" = "--write" ]; then
  cp dist-explorer/explorer.js dist-explorer/explorer.css "$vendor/explorer/"
  js=$(sum "$vendor/explorer/explorer.js"); css=$(sum "$vendor/explorer/explorer.css")
  head=$(git rev-parse HEAD)
  build="\`pnpm exec vite build -c vite.explorer.config.ts\`"
  printf '# monorepo joinedcontext: crates/portal/ui of %s, %s (reproducible)\n%s  explorer.css\n%s  explorer.js\n' "$head" "$build" "$css" "$js" > "$vendor/explorer/SHA256SUMS"
  sed -i "s|^[0-9a-f]\{64\}  explorer/explorer.css$|$css  explorer/explorer.css|; s|^[0-9a-f]\{64\}  explorer/explorer.js$|$js  explorer/explorer.js|" "$vendor/SHA256SUMS"
  echo "explorer-bundle: written ($js explorer.js)"
  exit 0
fi
bad=0
for f in explorer.js explorer.css; do
  if ! cmp -s "dist-explorer/$f" "$vendor/explorer/$f"; then
    echo "explorer-bundle: vendor/viz/explorer/$f is not what crates/portal/ui builds; run scripts/explorer-bundle.sh --write and commit it" >&2
    bad=1
  fi
  s=$(sum "dist-explorer/$f")
  grep -q "^$s  $f$" "$vendor/explorer/SHA256SUMS" || { echo "explorer-bundle: vendor/viz/explorer/SHA256SUMS does not hold $f's sum" >&2; bad=1; }
  grep -q "^$s  explorer/$f$" "$vendor/SHA256SUMS" || { echo "explorer-bundle: vendor/viz/SHA256SUMS does not hold explorer/$f's sum" >&2; bad=1; }
done
[ "$bad" = 0 ] && echo "explorer-bundle: the committed bundle is this tree's build"
exit "$bad"
