#!/usr/bin/env bash
# T-6454: turns the clips of clips_*.spec.ts into mp4 and judge stills.
#   clips.sh <clip dir> <out dir>
# Sandbox ffmpeg (Playwright's build) encodes VP8 only, so the mp4 comes from ffmpeg in a container.
# <out dir>/<journey>-<width>.mp4 + .vtt + .json, and stills/<journey>-<width>/ with one PNG every
# 2 s for the UI judge (KNOWLEDGE T-6092).
set -euo pipefail
in=$(realpath "${1:?clip dir}")
mkdir -p "${2:?out dir}"
out=$(realpath "$2")
cp "$in"/*.vtt "$in"/*.json "$out"/
docker run --rm -v "$in:/in:ro" -v "$out:/out" alpine:3.22 sh -ec '
  apk add --no-cache ffmpeg >/dev/null
  for f in /in/*.webm; do
    n=$(basename "$f" .webm)
    ffmpeg -y -v error -i "$f" -c:v libx264 -pix_fmt yuv420p -crf 26 -preset veryfast -movflags +faststart "/out/$n.mp4"
    mkdir -p "/out/stills/$n"
    ffmpeg -y -v error -i "$f" -vf fps=0.5 "/out/stills/$n/%03d.png"
  done
  chown -R "$(stat -c %u:%g /out)" /out
'
