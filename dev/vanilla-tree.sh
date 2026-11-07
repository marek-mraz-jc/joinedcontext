#!/usr/bin/env bash
# The default build's feature tree (ADR-0027 section 3 proof 1): what a vanilla
# broker compiles, dependency by dependency and feature by feature. CI diffs it
# against ci/vanilla-tree.txt, so a change to what vanilla links lands only
# with the file that shows it:
#
#   dev/vanilla-tree.sh > ci/vanilla-tree.txt     after a deliberate change
#
# One target, so an x86 and an arm host print the same tree; workspace paths
# are dropped, so the checkout's location is not part of it.
# --color never: CI sets CARGO_TERM_COLOR=always, which would put ANSI codes in the tree.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo tree --locked --color never -e features -p antares-broker --target x86_64-unknown-linux-gnu \
  | sed -E 's# \(/[^)]*\)##'
