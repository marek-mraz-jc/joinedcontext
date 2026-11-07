#!/usr/bin/env bash
# The surface crates' benches against the ADR-0027 section 6 budget rows
# (surfaces-test-plan section 10, T-3523): one row per bench, its verdict
# line and whether the number is within its row.
#
#   dev/perf/bench-surfaces.sh            # the nightly set
#   WEEKLY=1 dev/perf/bench-surfaces.sh   # + WebSocket fan-out and resident memory
#
# Report-only: no budget fails the run until T-3467's noise profile (ten
# same-commit runs on the pinned perf-weekly instance type,
# docs/src/performance.md) exists; until then a row past its budget is
# reported as `over`. ENFORCE=1 makes such a row fail the run once it
# does. A bench that does not run at all always fails it.
#
# Env: OUT (results/bench-surfaces), WEEKLY, ENFORCE.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT="${OUT:-results/bench-surfaces}"
mkdir -p "$OUT"

# row | crate | bench | the line that carries the number
BENCHES=(
  "NGSI-LD view: p50 ≤ 1.05×, p99 ≤ 1.10× of the root|antares-ops|view_query|view / root"
  "view load and composition (no row; reported)|antares-ops|views|"
  "OData read: p50 ≤ vanilla + 0.15 ms + 1 µs/Entity, p99 ≤ 1.25×|antares-odata|render|budget OData"
  "MCP tool call: ≤ vanilla + 0.2 ms + 1 µs/Entity|antares-mcp|call|budget MCP"
  "\$filter parse (no row; reported)|odata-url|parse|"
  "\$metadata build (LinkML document row is a 100-class schema; this fixture is reported)|odata-csdl|metadata|"
)
if [ -n "${WEEKLY:-}" ]; then
  BENCHES+=("WebSocket: socket p99 ≤ HTTP p99, ≤ 32 KiB per idle connection|antares-ws|fanout|")
fi

over=0
broken=0
md="$OUT/bench-surfaces.md"
{
  echo "| budget row | bench | number | verdict |"
  echo "|---|---|---|---|"
} > "$md"
for entry in "${BENCHES[@]}"; do
  IFS='|' read -r row crate bench line <<<"$entry"
  log="$OUT/$crate-$bench.log"
  status=0
  cargo bench --locked -q -p "$crate" --bench "$bench" > "$log" 2>&1 || status=$?
  if [ -n "$line" ]; then
    number=$(grep -F "$line" "$log" | tail -1 || true)
  else
    number=$(grep -E 'µs|ns/|KiB|p99' "$log" | tail -3 | paste -sd ';' - || true)
  fi
  # view_query exits non-zero when it misses its row; any other non-zero
  # exit, or a bench that printed nothing, did not run
  if grep -qE ' over\b|OVER|misses its ADR-0027|HTTP p99: false' "$log"; then
    verdict=over
    over=$((over + 1))
  elif [ "$status" -ne 0 ] || [ -z "$number" ]; then
    verdict="did not run (exit $status, $log)"
    broken=$((broken + 1))
  elif grep -qE 'within' <<<"$number"; then
    verdict=within
  else
    verdict=reported
  fi
  echo "| $row | \`$crate --bench $bench\` | ${number//|/\\|} | $verdict |" >> "$md"
done
{
  echo
  echo "Internal CSR forward (p99 ≤ 0.5× the HTTP forward) has no bench: no crate serves it yet."
  if [ -n "${ENFORCE:-}" ]; then
    echo "Enforced: a row past its budget fails the run."
  else
    echo "Report-only: no noise profile on the perf-weekly instance type yet (docs/src/performance.md)."
  fi
} >> "$md"
cat "$md"

[ "$broken" -eq 0 ] || { echo "$broken bench(es) did not run" >&2; exit 1; }
[ -z "${ENFORCE:-}" ] || [ "$over" -eq 0 ] || { echo "$over row(s) over budget" >&2; exit 1; }
