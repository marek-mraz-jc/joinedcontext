#!/usr/bin/env bash
# Which `portal-ui` bundle, or with --binary which Portal binary, goes with a portal commit
# (T-2485, T-5297, T-6158).
#
# usage: scripts/portal-ui-pin.sh [--binary] <portal commit, 40 hex>   -> prints "<image commit> <digest>"
#
# image.yml builds the bundle only for a push that changes ui/ or sdk/, and the binary only for a
# push that changes anything outside ui/ (dev/image-matrix.py), so a commit may publish one of the
# two. Then the image pinned now stays, or else the newest one published between the two, but
# only when no file it is built from changed between its commit and this one: for the bundle a
# file under ui/ or sdk/ (a bundle built from other files would call routes the binary may not
# have), for the binary any file outside ui/ (a ui/-only commit moves the bundle alone and dev
# rolls out portal-ui alone). Reads the public repository's compare API; GITHUB_TOKEN, when set,
# lifts its rate limit.
set -euo pipefail

part=ui
image=ghcr.io/marek-mraz-jc/joinedcontext-portal-ui
if [ "${1:-}" = --binary ]; then
  part=portal
  image=ghcr.io/marek-mraz-jc/joinedcontext-portal
  shift
fi
commit=${1:-}
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "usage: $0 [--binary] <portal commit, 40 hex>" >&2; exit 2; }
cd "$(dirname "$0")/.."
label=portal-ui
[ "$part" = ui ] || label="portal binary"

built=$({ docker buildx imagetools inspect "$image:$commit" 2>/dev/null || true; } | awk '/^Digest:/ { print $2; exit }')
if [[ "$built" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "$commit $built"
  exit 0
fi

# The `ui:` (or `portal:`) block of components/portal/images.yaml: its digest and the commit it was built from.
read -r pinned digest < <(awk -v block="  $part:" '$0 == block { mine = 1; next } mine && /^  [a-z]/ { exit }
  mine && /^    digest:/ { gsub(/\x27/, "", $2); d = $2 } mine && /^    commit:/ { gsub(/\x27/, "", $2); c = $2 }
  END { print c, d }' components/portal/images.yaml)
[[ "$pinned" =~ ^[0-9a-f]{40}$ && "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] ||
  { echo "image.yml published no $label for $commit and none is pinned" >&2; exit 1; }
[ "$pinned" = "$commit" ] && { echo "$pinned $digest"; exit 0; }

auth=()
[ -n "${GITHUB_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# holds <ui commit> <its name>: exit 0 when the bundle built from <ui commit> fits $commit; else the reason on
# stderr and, in $work/candidates, the compare's commits newest first.
holds() {
  # The answer goes through a file: a compare past ~128 KiB exceeds one environment string.
  curl -fsSL --max-time 30 "${auth[@]}" -o "$work/compare" \
    "https://api.github.com/repos/marek-mraz-jc/joinedcontext-portal/compare/$1...$commit?per_page=300" 2>/dev/null ||
    { echo "the compare of portal ${1:0:7}...${commit:0:7} did not answer; nothing decided" >&2; exit 1; }
  python3 - "$1" "$commit" "$work/compare" "$work/candidates" "$2" "$label" <<'PY'
import json
import sys

base, commit, name, label = sys.argv[1][:7], sys.argv[2][:7], sys.argv[5], sys.argv[6]
with open(sys.argv[3], encoding="utf-8") as f:
    answer = json.load(f)
with open(sys.argv[4], "w", encoding="utf-8") as f:
    f.writelines(c["sha"] + "\n" for c in reversed(answer.get("commits", [])) if c["sha"] != sys.argv[2])
if answer.get("status") not in ("ahead", "identical"):
    sys.exit(f"portal {commit} does not descend from {name} {base} ({answer.get('status')})")
files = [f["filename"] for f in answer.get("files", [])]
# The API lists at most 300 files: past that a change may hide in the rest.
if len(files) >= 300:
    sys.exit(f"portal {base}...{commit} changed 300 files or more; pin a commit image.yml built the {label} for")
if label == "portal-ui":
    changed = [f for f in files if f.startswith(("ui/", "sdk/"))]
else:
    changed = [f for f in files if not f.startswith("ui/")]
if changed:
    sys.exit(f"portal {commit} changed {changed[0]} since {name} {base} and image.yml "
             f"published no {label} for it")
PY
}

holds "$pinned" "the pinned $label" && { echo "$pinned $digest"; exit 0; }

# The pinned image does not fit, but a newer commit of the compare may have published one that
# was never pinned (T-6050). Only the newest published one can fit: every change it is built from
# after it also comes after each older one.
cp "$work/candidates" "$work/walk"
while read -r candidate; do
  [[ "$candidate" =~ ^[0-9a-f]{40}$ ]] || continue
  found=$({ docker buildx imagetools inspect "$image:$candidate" 2>/dev/null || true; } | awk '/^Digest:/ { print $2; exit }')
  [[ "$found" =~ ^sha256:[0-9a-f]{64}$ ]] || continue
  holds "$candidate" "the newer $label" || { echo "the newest $label since, ${candidate:0:7}, does not fit either" >&2; exit 1; }
  echo "$candidate $found"
  exit 0
done <"$work/walk"
exit 1
