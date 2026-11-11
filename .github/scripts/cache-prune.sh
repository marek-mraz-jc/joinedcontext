#!/bin/sh
# Deletes the Swatinem/rust-cache entries (.github/actions/rust) a newer save of the same build
# superseded: a key is v0-rust-<build>-<os>-<arch>-<env hash>-<lockfile hash>, a new lockfile or
# toolchain saves a new key and the old one stays until eviction (941 MB each for root-test). Per
# ref and build only the newest entry stays. It also drops the layer cache (type=gha) a newer
# export superseded: every index-<scope>#<n> but the newest of its scope, and every buildkit-blob
# last read before the second-newest index of each scope was written, less 2 h for exports of one
# scope that overlap (image-embedder runs one per commit). An export reads or writes each blob its
# index lists, so such a blob is in no newest index; while a scope has one index nothing is
# dropped. Each image-embedder export writes ~140 MB of new blobs that the next one supersedes,
# and the 10 GB cap's eviction took the 607 MB portal-test target cache (oldest read) before them.
# sccache keeps its entries: content-addressed, it has no superseded entry the API can tell.
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

# layers: `<id> <ref> <key> <created> <last read>` lines on stdin -> the ids of superseded layer
# cache entries
layers() {
  awk '$3 ~ /^index-/ { g = $3; sub(/#[0-9]+$/, "", g); n = $3; sub(/.*#/, "", n); n += 0; idx[NR] = $1 " " g " " n " " $4
      if (n > top[g]) { second[g] = at[g]; top[g] = n; at[g] = $4 } else if ($4 > second[g]) second[g] = $4 }
    $3 ~ /^buildkit-blob-/ { blob[$1] = $5 }
    END {
      for (i in idx) { split(idx[i], f, " "); if (f[3] < top[f[2]]) print f[1] }
      for (g in top) { if (second[g] == "") one = 1; if (cut == "" || second[g] < cut) cut = second[g] }
      if (one || cut == "") exit
      # cut less 2 h, as an ISO time string compares
      cmd = "date -u -d \"" cut " - 2 hours\" +%Y-%m-%dT%H:%M:%SZ"; cmd | getline cut; close(cmd)
      for (b in blob) if (blob[b] < cut) print b
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
  ix() { printf '%s refs/heads/jc-portal-1 %s %s %s\n' "$1" "$2" "$3" "$3"; }
  bl() { printf '%s refs/heads/jc-portal-1 buildkit-blob-1-sha256:%s 2026-11-11T00:00:00Z %s\n' "$1" "$1" "$2"; }
  two=$(ix 10 'index-jc-1-9e92e7d1#3' 2026-11-11T04:25:00Z; ix 11 'index-jc-1-9e92e7d1#2' 2026-11-11T04:03:00Z
    ix 12 'index-jc-1-9e92e7d1#1' 2026-11-11T02:07:00Z; ix 13 'index-jc-embedder-1-9e92e7d1#6' 2026-11-11T06:35:00Z
    ix 14 'index-jc-embedder-1-9e92e7d1#5' 2026-11-11T05:48:00Z
    bl 20 2026-11-11T02:02:59Z; bl 21 2026-11-11T02:03:01Z; bl 22 2026-11-11T06:28:00Z
    printf '%s\n' '30 refs/heads/jc-portal-1 sccache/0/0/0/00 2026-11-01T00:00:00Z 2026-11-01T00:00:00Z' \
      '31 refs/heads/jc-portal-1 v0-rust-root-test-Linux-x64-43996dc4-2d045d6b 2026-11-01T00:00:00Z 2026-11-01T00:00:00Z')
  out=$(echo "$two" | layers | sort | tr '\n' ' ')
  [ "$out" = "11 12 14 20 " ] || { echo "FAIL: layers gave '$out', want '11 12 14 20 '"; exit 1; }
  # a scope with one index (a first export): only the older indexes of the others go
  out=$( (echo "$two"; ix 15 'index-jc-app-x-1-9e92e7d1#1' 2026-11-11T06:00:00Z) | layers | sort | tr '\n' ' ')
  [ "$out" = "11 12 14 " ] || { echo "FAIL: layers with a one-index scope gave '$out', want '11 12 14 '"; exit 1; }
  echo "cache-prune selftest: ok"
  exit 0
fi

repo=${GITHUB_REPOSITORY:?}
all=$(gh api --paginate "repos/$repo/actions/caches?per_page=100" \
  -q '.actions_caches[] | "\(.id) \(.ref) \(.key) \(.created_at) \(.last_accessed_at)"')
echo "$all" | stale | while read -r id; do
  echo "delete superseded rust-cache entry $id"
  gh api -X DELETE "repos/$repo/actions/caches/$id" >/dev/null
done
echo "$all" | layers | while read -r id; do
  echo "delete superseded layer cache entry $id"
  gh api -X DELETE "repos/$repo/actions/caches/$id" >/dev/null
done
