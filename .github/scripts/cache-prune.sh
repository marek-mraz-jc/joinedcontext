#!/bin/sh
# Deletes the Swatinem/rust-cache entries (.github/actions/rust) a newer save of the same build
# superseded: a key is v0-rust-<build>-<os>-<arch>-<env hash>-<lockfile hash>, a new lockfile or
# toolchain saves a new key and the old one stays until eviction (941 MB each for root-test). Per
# ref and build only the newest entry stays. Nothing else is touched: the layer cache (buildkit-*,
# index-*) and sccache keep their entries.
#   GH_TOKEN=... sh cache-prune.sh     # in dockerfile-cache.yml (actions: write)
#   sh cache-prune.sh --selftest
set -eu

# stale: `<id> <ref> <key> <created>` lines on stdin -> the ids of superseded rust-cache entries
stale() {
  awk '$3 ~ /^v0-rust-/ {
      g = $3; sub(/-[^-]+-[^-]+$/, "", g); g = $2 " " g
      if (!(g in at) || $4 > at[g]) { if (g in id) print id[g]; at[g] = $4; id[g] = $1 }
      else print $1
    }'
}

if [ "${1:-}" = --selftest ]; then
  out=$(printf '%s\n' \
    '1 refs/heads/jc-portal-1 v0-rust-root-test-Linux-x64-43996dc4-2d045d6b 2026-11-09T11:19:00Z' \
    '2 refs/heads/jc-portal-1 v0-rust-root-test-Linux-x64-43996dc4-d618b189 2026-11-09T14:50:00Z' \
    '3 refs/heads/jc-portal-1 v0-rust-root-clippy-Linux-x64-43996dc4-d618b189 2026-11-09T14:46:00Z' \
    '4 refs/heads/main v0-rust-root-test-Linux-x64-43996dc4-aaaa0000 2026-11-09T10:00:00Z' \
    '5 refs/heads/jc-portal-1 v0-rust-root-test-Linux-x64-11112222-bbbb0000 2026-11-09T09:00:00Z' \
    '6 refs/heads/jc-portal-1 buildkit-blob-1-sha256:00 2026-11-01T00:00:00Z' \
    '7 refs/heads/jc-portal-1 sccache/0/0/0/00 2026-11-01T00:00:00Z' \
    '8 refs/heads/jc-portal-1 index-jc-1-9e92e7d1#1 2026-11-01T00:00:00Z' | stale | sort | tr '\n' ' ')
  [ "$out" = "1 5 " ] || { echo "FAIL: stale gave '$out', want '1 5 '"; exit 1; }
  echo "cache-prune selftest: ok"
  exit 0
fi

repo=${GITHUB_REPOSITORY:?}
gh api --paginate "repos/$repo/actions/caches?per_page=100&key=v0-rust-" \
  -q '.actions_caches[] | "\(.id) \(.ref) \(.key) \(.created_at)"' | stale | while read -r id; do
  echo "delete superseded rust-cache entry $id"
  gh api -X DELETE "repos/$repo/actions/caches/$id" >/dev/null
done
