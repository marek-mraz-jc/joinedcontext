#!/usr/bin/env bash
# SPDX-License-Identifier: EUPL-1.2
# The new-surface template still makes a surface that builds, passes its generated tests
# (the Policy deny test among them), lints clean and keeps module rule M2 (T-5063,
# docs/src/new-surface.md): generated into a scratch worktree, never into this tree.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}"

python3 dev/new-surface.py --self-test

tmp=$(mktemp -d)
cleanup() {
  git -C "$repo" worktree remove --force "$tmp/tree" >/dev/null 2>&1 || true
  rm -rf "${tmp:?}"
}
trap cleanup EXIT
git worktree add --detach -q "$tmp/tree" HEAD
# the template as this tree has it, committed or not
cp -r dev/new-surface dev/new-surface.py "$tmp/tree/dev/"
cd "$tmp/tree"

python3 dev/new-surface.py smoke --title "Smoke" --description "the smoke test's surface."
cargo test -p antares-smoke
cargo clippy -p antares-smoke --all-targets -- -D warnings
python3 dev/check-module-rules.py
cargo check -p antares-broker --features smoke
echo "new-surface smoke: OK"
