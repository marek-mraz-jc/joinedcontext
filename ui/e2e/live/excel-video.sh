#!/usr/bin/env bash
# T-6567: the presentation take of excel_one_go.spec.ts (EXCEL_VIDEO=<dir>) without its waits.
#   excel-video.sh <take dir> <out dir>
# <take dir> holds flow.webm and cuts.json ([[from, to], ...] seconds the spec waited: sign-in,
# approval, the row write, the space page load, the model's thinking). Writes <out dir>/excel-one-go.mp4
# and .webm with every other second kept. Sandbox ffmpeg encodes VP8 only, so ffmpeg runs in a container.
set -euo pipefail
in=$(realpath "${1:?take dir}")
mkdir -p "${2:?out dir}"
out=$(realpath "$2")
# The kept parts as an ffmpeg select expression: not between any cut.
keep=$(python3 -I -c '
import json, sys
cuts = json.load(open(sys.argv[1]))
print("not(" + "+".join(f"between(t,{a:.1f},{b:.1f})" for a, b in cuts) + ")" if cuts else "1")
' "$in/cuts.json")
docker run --rm -v "$in:/in:ro" -v "$out:/out" -e KEEP="$keep" alpine:3.22 sh -ec '
  apk add --no-cache ffmpeg >/dev/null
  vf="select='"'"'$KEEP'"'"',setpts=N/FRAME_RATE/TB"
  ffmpeg -y -v error -i /in/flow.webm -an -vf "$vf" -r 25 -c:v libx264 -pix_fmt yuv420p -crf 23 -preset veryfast -movflags +faststart /out/excel-one-go.mp4
  ffmpeg -y -v error -i /out/excel-one-go.mp4 -c:v libvpx -b:v 1M -crf 10 /out/excel-one-go.webm
  chown -R "$(stat -c %u:%g /out)" /out
'
docker run --rm -v "$out:/out:ro" alpine:3.22 sh -ec 'apk add --no-cache ffmpeg >/dev/null; ffprobe -v error -show_entries format=duration -of csv=p=0 /out/excel-one-go.mp4'
