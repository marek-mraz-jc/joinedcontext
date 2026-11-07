#!/usr/bin/env bash
# Cuts the release tag of CONTRIBUTING.md's "Promotion to main", step 4:
# tags origin/main with the workspace version that commit carries, shows
# what it is about to push, and pushes on a typed yes. The same two rules
# dev/check-source.sh holds in full.yml are checked here first, because a
# pushed v* tag can be neither moved nor deleted (protect-tags ruleset).
#
#   dev/release-tag.sh            tag, confirm, push
#   dev/release-tag.sh --dry-run  every check, nothing created or pushed
#
# Signs the tag when git has a usable signing key, otherwise annotates it;
# nothing downstream verifies the tag signature (the image is signed by
# cosign in full.yml).
set -euo pipefail

dry=0
case "${1:-}" in
  "") ;;
  --dry-run) dry=1 ;;
  *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
esac

cd "$(git rev-parse --show-toplevel)"
git fetch --quiet --tags origin main

version=$(git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
[ -n "$version" ] || { echo "no workspace version in origin/main:Cargo.toml" >&2; exit 1; }
tag="v$version"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
   [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
  echo "$tag already exists; bump Cargo.toml on dev and promote before tagging again" >&2
  exit 1
fi

# The promotion merges with a merge commit; anything else on top of main
# means the pull request was squashed, rebased, or main was pushed to.
if [ "$(git rev-list --no-walk --count --min-parents=2 origin/main)" != 1 ]; then
  echo "origin/main is not a merge commit; tag only the promotion's merge" >&2
  exit 1
fi

echo "tag     $tag"
echo "commit  $(git log -1 --format='%h %s (%cr)' origin/main)"

if [ "$dry" = 1 ]; then echo "dry run: nothing tagged, nothing pushed"; exit 0; fi

read -r -p "Push $tag? It cannot be moved or deleted afterwards. Type yes: " answer
[ "$answer" = yes ] || { echo "stopped; nothing tagged"; exit 1; }

# A failed signature leaves no tag behind, so the annotated retry is clean.
if git tag -s "$tag" -m "$tag" origin/main 2>/dev/null; then
  echo "signed tag created"
else
  git tag -a "$tag" -m "$tag" origin/main
  echo "annotated tag created (no usable signing key)"
fi
git push origin "$tag"
echo "pushed; full.yml now runs the release gate for $tag"
