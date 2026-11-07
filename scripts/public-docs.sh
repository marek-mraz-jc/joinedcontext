#!/usr/bin/env bash
# The public part of the documentation (T-4063): the API chapter and the User Guide, which the
# Portal serves at /docs/{chapter}/{page} to anyone, signed in or not. Bundled at build time from
# a pinned docs commit, like ui/src/generated/guideSections.json: no runtime fetch, and the words
# match this Portal's version. No other chapter is copied, so none can be served.
#
#   scripts/public-docs.sh <docs-clone> <commit>   rewrites public-docs/ from that commit
#   scripts/public-docs.sh --check                 fails unless public-docs/ is exactly the docs
#                                                   commit public-docs/COMMIT names (CI, T-4483)
#
# The deployment pins the commit beside the Portal's digest and checks those docs against the
# code it deploys (T-4483), so the folder is never edited by hand: CI refuses one that differs
# from its commit by a byte.
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/public-docs"

# The chapters of the tree at $1 (a checkout of the docs repository) into $2.
copy() {
	for chapter in API User-Guide; do
		mkdir -p "$2/$chapter"
		find "$1/$chapter" -maxdepth 1 -type f -name '*.md' -exec cp {} "$2/$chapter/" \;
	done
}

if [ "${1:-}" = --check ]; then
	commit=$(cat "$out/COMMIT")
	[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "public-docs/COMMIT is not one commit: $commit" >&2; exit 1; }
	work=$(mktemp -d)
	trap 'rm -rf "${work:?}"' EXIT
	mkdir -p "$work/docs" "$work/public-docs"
	curl -fsSL "https://codeload.github.com/marek-mraz-jc/joinedcontext-docs/tar.gz/$commit" |
		tar -xz -C "$work/docs" --strip-components=1
	copy "$work/docs" "$work/public-docs"
	echo "$commit" >"$work/public-docs/COMMIT"
	diff -r "$work/public-docs" "$out" || {
		echo "public-docs/ is not docs $commit: regenerate it with scripts/public-docs.sh <docs-clone> <commit>" >&2
		exit 1
	}
	echo "public-docs/ is docs $commit"
	exit 0
fi

[ $# -eq 2 ] || { echo "usage: $0 <docs-clone> <commit> | --check" >&2; exit 64; }
clone=$1
commit=$(git -C "$clone" rev-parse --verify "$2^{commit}")
work=$(mktemp -d)
trap 'rm -rf "${work:?}"' EXIT
git -C "$clone" archive "$commit" API User-Guide | tar -x -C "$work"
rm -rf "${out:?}"
mkdir -p "$out"
copy "$work" "$out"
echo "$commit" >"$out/COMMIT"
