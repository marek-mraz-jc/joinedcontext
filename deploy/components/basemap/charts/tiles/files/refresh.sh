#!/bin/sh
# T-4638: the basemap's monthly refresh. It extracts the regions from the newest Protomaps
# daily build into a new file beside the old one, after checking the volume holds it, and
# swaps it in with one rename, so the server never reads half a file. A failure at any step
# leaves the old file as it was. The fonts and sprites are fetched once per pinned commit.
set -eu

: "${DATA:?}" "${FILE:?}" "${MAXZOOM:?}" "${REGION:?}" "${BUILDS:?}"
: "${PMTILES_VERSION:?}" "${PMTILES_SHA256:?}" "${ASSETS_COMMIT:?}" "${ASSETS_SHA256:?}"
cd /tmp

# The CLI, pinned by version and checked by its hash before it runs.
wget -q -O pmtiles.tgz \
  "https://github.com/protomaps/go-pmtiles/releases/download/v${PMTILES_VERSION}/go-pmtiles_${PMTILES_VERSION}_Linux_x86_64.tar.gz"
echo "${PMTILES_SHA256}  pmtiles.tgz" | sha256sum -c -
tar -xzf pmtiles.tgz pmtiles

# The newest daily build of the last week (a build appears some hours after its date).
build=""
now=$(date -u +%s)
for back in 0 1 2 3 4 5 6 7; do
  day=$(date -u -d "@$((now - back * 86400))" +%Y%m%d)
  if wget -q --spider "${BUILDS}/${day}.pmtiles"; then
    build="${BUILDS}/${day}.pmtiles"
    break
  fi
done
[ -n "$build" ] || { echo "no Protomaps build in the last 8 days at ${BUILDS}" >&2; exit 1; }
echo "build: $build"

# The regions as one GeoJSON MultiPolygon, built from values by the chart.
printf '%s' "$REGION" > region.geojson
echo "region: $(cat region.geojson)"

# The archive's size before a byte is written, as the CLI reports it ("archive size of 5.0 GB").
size=$(./pmtiles extract "$build" /dev/null --region=region.geojson --maxzoom="$MAXZOOM" --dry-run 2>&1 \
  | sed -n 's/.*archive size of \([0-9.]*\) \([kMGT]*B\).*/\1 \2/p' | tail -1 \
  | awk '{ m = 1; if ($2 == "kB") m = 1e3; if ($2 == "MB") m = 1e6; if ($2 == "GB") m = 1e9; if ($2 == "TB") m = 1e12
           printf "%.0f", $1 * m }')
[ -n "$size" ] || { echo "the dry run reported no archive size" >&2; exit 1; }
df -P "$DATA" | tail -1 | sed 's/^/df: /'
free=$(df -P "$DATA" | awk 'NR == 2 { printf "%.0f", $4 * 1024 }')
need=$(awk -v s="$size" 'BEGIN { printf "%.0f", s * 1.2 }')
echo "expected ${size} bytes, need ${need} with 20 % room, free ${free}"
if [ "$need" -gt "$free" ]; then
  echo "not enough room on ${DATA} for the new extract; the old file stays" >&2
  exit 1
fi

tmp="${DATA}/.${FILE}.tmp"
rm -f "$tmp"
./pmtiles extract "$build" "$tmp" --region=region.geojson --maxzoom="$MAXZOOM" --quiet
./pmtiles verify "$tmp"
mv -f "$tmp" "${DATA}/${FILE}"
echo "$build" > "${DATA}/${FILE}.build"
ls -l "${DATA}/${FILE}"

# Fonts and sprites, once per pinned commit of protomaps/basemaps-assets.
if [ ! -f "${DATA}/.assets-${ASSETS_COMMIT}" ]; then
  wget -q -O assets.tgz "https://codeload.github.com/protomaps/basemaps-assets/tar.gz/${ASSETS_COMMIT}"
  echo "${ASSETS_SHA256}  assets.tgz" | sha256sum -c -
  rm -rf "${DATA}/.assets.tmp" && mkdir "${DATA}/.assets.tmp"
  tar -xzf assets.tgz -C "${DATA}/.assets.tmp" --strip-components=1 \
    "basemaps-assets-${ASSETS_COMMIT}/fonts" "basemaps-assets-${ASSETS_COMMIT}/sprites"
  for dir in fonts sprites; do
    rm -rf "${DATA}/${dir}.old"
    [ ! -d "${DATA}/${dir}" ] || mv "${DATA}/${dir}" "${DATA}/${dir}.old"
    mv "${DATA}/.assets.tmp/${dir}" "${DATA}/${dir}"
    rm -rf "${DATA}/${dir}.old"
  done
  rm -rf "${DATA}/.assets.tmp" "${DATA}"/.assets-*
  touch "${DATA}/.assets-${ASSETS_COMMIT}"
fi
echo "done"
