#!/usr/bin/env bash
# Commits one perf run's tables to the `perf-history` branch (T-4853), so a
# run's numbers outlive the 30-90 days an Actions artifact is kept (run
# 38135751509, the one docs/src/perf-baseline.md cites, is already gone).
#
#   dev/perf/commit-history.sh <results dir> <label>
#
# Copies the run's .md, .json and .txt files (no logs) to
# runs/<UTC time>-<label>-<sha>/ on perf-history, creating the branch on
# its first run. Needs `contents: write`; the caller's checkout supplies
# the push credentials. Synthetic data only: the results hold timings.
set -euo pipefail

src="${1:?results dir}"
label="${2:?label}"
[ -d "$src" ] || { echo "no results in $src" >&2; exit 1; }
[[ "$label" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "label $label: letters, digits, . _ - only" >&2; exit 1; }
sha="$(git rev-parse --short=7 HEAD)"
dest="runs/$(date -u +%Y-%m-%dT%H%MZ)-$label-$sha"
wt="$(mktemp -d)"
trap 'git worktree remove --force "$wt" 2>/dev/null || true' EXIT

# stage: the run's files as one commit on top of the branch as fetched now
stage() {
  if git fetch -q --depth 1 origin +perf-history:refs/remotes/origin/perf-history 2>/dev/null; then
    git -C "$wt" checkout -q -f --detach refs/remotes/origin/perf-history
  else
    git -C "$wt" checkout -q -f --orphan "perf-history-new-$1"
    git -C "$wt" rm -rqf --ignore-unmatch .
  fi
  git -C "$wt" clean -qfd
  mkdir -p "$wt/$dest"
  (cd "$src" && find . -type f \( -name '*.md' -o -name '*.json' -o -name '*.txt' \) -print0) \
    | while IFS= read -r -d '' f; do
        mkdir -p "$wt/$dest/$(dirname "$f")"
        cp "$src/$f" "$wt/$dest/$f"
      done
  git -C "$wt" add "$dest"
  git -C "$wt" -c user.name="antares-perf" -c user.email="perf@users.noreply.github.com" \
    commit -qm "perf $label at $sha"
}

git worktree add -q --detach "$wt"
for attempt in 1 2 3; do
  stage "$attempt"
  git -C "$wt" push -q origin HEAD:refs/heads/perf-history && { echo "committed $dest to perf-history"; exit 0; }
done
echo "perf-history push refused three times" >&2
exit 1
