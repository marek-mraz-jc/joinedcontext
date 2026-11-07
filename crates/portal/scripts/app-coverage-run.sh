#!/bin/sh
# One App under the coverage gate (T-3373), run from the App's own folder after its install:
#
#   sh scripts/app-coverage-run.sh <app-name> [--rust]
#
# Runs the App's vitest suite with v8 coverage (the coverage package at the version of the App's
# own vitest, added to this throwaway copy only) and the SDK's controls record, then, with
# `--rust`, `cargo llvm-cov` of its `wasm/` crate, and hands the reports to the gate, which fails
# the App below the thresholds of apps/coverage-thresholds.json unless apps/coverage-pending.txt
# names its task. The suite's own failures fail here as `pnpm test` did.
set -eu

NAME=${1:?usage: app-coverage-run.sh <app-name> [--rust]}
RUST=${2:-}
GATE=$(cd "$(dirname "$0")" && pwd)/app-coverage-gate.mjs
OUT=$(mktemp -d)

VITEST=$(node -p "require('vitest/package.json').version")
pnpm add --save-dev "@vitest/coverage-v8@$VITEST" >/dev/null

JC_CONTROLS_DIR="$OUT/controls" pnpm exec vitest run \
  --coverage.enabled --coverage.provider=v8 \
  --coverage.reporter=json-summary --coverage.reporter=text-summary \
  --coverage.reportsDirectory="$OUT/coverage"

set -- --coverage "$OUT/coverage/coverage-summary.json" --controls "$OUT/controls"
if [ "$RUST" = "--rust" ] && [ -f wasm/Cargo.toml ]; then
  (cd wasm && cargo llvm-cov --locked --json --summary-only --output-path "$OUT/rust.json")
  set -- "$@" --rust "$OUT/rust.json"
fi

node "$GATE" app "$NAME" "$@"
