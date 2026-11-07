#!/bin/sh
# The User Guide's screenshots from a live run into the docs repository (T-3270):
#
#   sh scripts/publish-guide-shots.sh ui/test-results/guide ../docs
#
# Takes every shot the journeys wrote with GUIDE_SHOTS=1 (`{en,sk}/{name}.png`, see
# ui/e2e/live/guide.ts), quantizes it with pngquant into `User-Guide/img/{lang}/{name}.png` and
# commits the change in the docs clone, once, when anything changed; pushing it is the caller's.
# A shot taken in one language only is refused: the guides are en and sk. `PNGQUANT` names the
# quantizer; without it, pngquant on the PATH, else the pinned npm build of it.
set -eu

src=${1:?usage: publish-guide-shots.sh <guide results dir> <docs clone>}
docs=${2:?usage: publish-guide-shots.sh <guide results dir> <docs clone>}
[ -d "$src/en" ] || { echo "publish-guide-shots: no $src/en: run the journeys with GUIDE_SHOTS=1 first" >&2; exit 1; }
[ -d "$docs/User-Guide" ] || { echo "publish-guide-shots: $docs is not a docs clone (no User-Guide/)" >&2; exit 1; }
if [ -n "${PNGQUANT:-}" ]; then quant=$PNGQUANT
elif command -v pngquant >/dev/null 2>&1; then quant=pngquant
else quant="npx -y pngquant-bin@9.0.0"; fi
# The size one shot may have after quantizing; a larger one is a page that needs a smaller shot.
max=${GUIDE_SHOT_MAX_BYTES:-307200}

count=0
for shot in "$src"/en/*.png; do
  [ -e "$shot" ] || continue
  name=$(basename "$shot" .png)
  printf '%s' "$name" | grep -Eq '^[a-z0-9]+(-[a-z0-9]+)*$' || { echo "publish-guide-shots: $name is not a shot name" >&2; exit 1; }
  [ -f "$src/sk/$name.png" ] || { echo "publish-guide-shots: $name was shot in en only; the guides are en and sk" >&2; exit 1; }
  for lang in en sk; do
    out="$docs/User-Guide/img/$lang/$name.png"
    mkdir -p "$(dirname "$out")"
    $quant --quality 50-90 --strip --force --output "$out" -- "$src/$lang/$name.png"
    size=$(wc -c < "$out")
    [ "$size" -le "$max" ] || { echo "publish-guide-shots: $lang/$name.png is $size bytes, over $max" >&2; exit 1; }
  done
  count=$((count + 1))
done
for shot in "$src"/sk/*.png; do
  [ -e "$shot" ] || continue
  [ -f "$src/en/$(basename "$shot")" ] || { echo "publish-guide-shots: $(basename "$shot" .png) was shot in sk only" >&2; exit 1; }
done
[ "$count" -gt 0 ] || { echo "publish-guide-shots: no shot in $src" >&2; exit 1; }

git -C "$docs" add User-Guide/img
if git -C "$docs" diff --cached --quiet; then
  echo "publish-guide-shots: $count shots, none changed"
else
  git -C "$docs" commit -q -m "User Guide: $count screenshots from the live journeys (T-3270)"
  echo "publish-guide-shots: $count shots, committed in $docs"
fi
