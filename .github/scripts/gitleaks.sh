#!/bin/sh
# gitleaks over the commits this event brings: a pull request's base..head, a push's before..sha,
# and for a new branch every commit no other remote branch holds. The imported trees' history
# (subtree squashes of Antares, ekiben, the Portal) carries upstream test vectors that their own
# repositories allow-list; scanning only new commits keeps every new leak a red run.
set -eu
VERSION=8.30.1
SHA256=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
zero=0000000000000000000000000000000000000000
url="https://github.com/gitleaks/gitleaks/releases/download/v${VERSION}/gitleaks_${VERSION}_linux_x64.tar.gz"
tmp=$(mktemp -d)
curl -fsSL "$url" -o "$tmp/gl.tgz"
echo "$SHA256  $tmp/gl.tgz" | sha256sum -c -
tar -xzf "$tmp/gl.tgz" -C "$tmp" gitleaks

case "${EVENT:-}" in
  pull_request) range="${PR_BASE}..HEAD" ;;
  push)
    # Commits another remote branch holds were scanned when that branch got them, and are public
    # there already: a fast-forward of main to jc-portal-1 scans only what neither held. No
    # trailing space: gitleaks splits --log-opts on single spaces, and an empty argument makes its
    # git log fail, which it reports as "no leaks found".
    others=$(git for-each-ref --format='%(refname)' refs/remotes/origin | grep -vx "refs/remotes/origin/${BRANCH}" | grep -vx refs/remotes/origin/HEAD | paste -sd' ' - || true)
    if [ -n "${BEFORE:-}" ] && [ "$BEFORE" != "$zero" ] && git cat-file -e "${BEFORE}^{commit}" 2>/dev/null; then
      range="${BEFORE}..HEAD${others:+ --not $others}"
    else
      range="HEAD --not $others"
    fi ;;
  # a dispatch: the newest commit
  *) range="HEAD~1..HEAD" ;;
esac
# A range git cannot read would scan nothing and pass: refuse it here.
# shellcheck disable=SC2086
n=$(git rev-list --count $range)
echo "gitleaks over: $range ($n commits)"
"$tmp/gitleaks" detect --no-banner --redact --log-opts="$range"
