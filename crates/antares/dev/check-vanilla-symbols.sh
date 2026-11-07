#!/usr/bin/env bash
# Rule M3, the linker half (surfaces-threat-model 9.5 items 2 and 3, ADR-0027
# section 3): a vanilla broker links no surface, ops or protocol crate.
#
#   dev/check-vanilla-symbols.sh                      the dev-profile binary
#   dev/check-vanilla-symbols.sh --audit-info <bin>   a shipped (stripped) one
#   dev/check-vanilla-symbols.sh --metadata           the resolved graph, no build
#
# The first mode builds `-p antares-broker` with the default features in the
# dev profile (release is stripped, F6), greps its demangled symbols for the
# crate paths below, and starts it with a surface it was not built with: it
# must refuse naming "unknown surface". Its self-test builds the same binary
# with an addon (`plugin-example`; `full` once it exists) and requires the
# grep to find it, so the gate is not a grep that never matched anything.
#
# The second mode reads the package list `cargo auditable` embeds
# (Dockerfile) with `rust-audit-info`; that checks the image's /antares
# itself, which has no symbols left to read.
#
# The third mode, `--sbom <cdx.json>`, reads the component names of a
# CycloneDX SBOM of the broker (dev/sbom.sh) the same way.
#
# The fourth mode, `--metadata`, builds nothing: it walks the packages
# `cargo metadata` resolves under `antares-broker` (normal and build
# edges), which is the list `cargo auditable` embeds. Cargo unifies features
# across the workspace there, so a surface that turns on a feature of a
# shared crate (axum's `ws`, T-3704) shows up here as it does in the image.
# The first mode runs it before building.
set -euo pipefail
cd "$(dirname "$0")/.."

# Rust paths, as `nm --demangle` prints them.
SYMBOLS='(^|[^A-Za-z0-9_])(antares_(ops|views|odata|ogc|mcp|linkml|ws|icsr|plugin_[a-z_]+)|joinedcontext_[a-z_]+|odata_(url|csdl|json|batch|axum)|linkml_model|ogc_cql2|rmcp|tungstenite)::'
# The platform's NGSI-LD extensions inside antares-api (feature `jc`, T-4854):
# the gateway's provenance headers and `versions=all`. No crate of their own,
# so the package modes cannot see them; this pattern is checked on the binary.
JC_SYMBOLS='antares_api::provenance::(may_read_versions|from_gateway|percent_decoded)'
# The same crates as package names, as the auditable list carries them.
PACKAGES='^(antares-(ops|views|odata|ogc|mcp|linkml|ws|icsr|plugin-.+)|joinedcontext-.+|odata-.+|linkml-model|ogc-cql2|rmcp|tungstenite)$'

if [ "${1:-}" = --sbom ]; then
  sbom=${2:?usage: --sbom <cdx.json>}
  names=$(jq -r '.components[]?.name' "$sbom")
  [ -n "$names" ] || { echo "no component in $sbom: the check would pass vacuously"; exit 1; }
  if found=$(printf '%s\n' "$names" | grep -E "$PACKAGES"); then
    echo "SURFACE IN THE VANILLA SBOM $sbom:"
    printf '%s\n' "$found"
    exit 1
  fi
  echo "vanilla SBOM: OK ($(printf '%s\n' "$names" | wc -l) components, none a surface)"
  exit 0
fi

metadata() {
  local packages
  packages=$(cargo metadata --locked --format-version 1 | python3 -c '
import json, sys
m = json.load(sys.stdin)
name = {p["id"]: p["name"] for p in m["packages"]}
deps = {n["id"]: [d["pkg"] for d in n["deps"]
                  if any(k["kind"] in (None, "build") for k in d["dep_kinds"])]
        for n in m["resolve"]["nodes"]}
todo = [i for i, n in name.items() if n == "antares-broker"]
seen = set(todo)
while todo:
    for d in deps.get(todo.pop(), []):
        if d not in seen:
            seen.add(d)
            todo.append(d)
print("\n".join(sorted({name[i] for i in seen})))
')
  printf '%s\n' "$packages" | grep -qx antares-broker || {
    echo "cargo metadata resolves no antares-broker: the check would pass vacuously"
    return 1
  }
  if found=$(printf '%s\n' "$packages" | grep -E "$PACKAGES"); then
    echo "SURFACE IN THE RESOLVED GRAPH OF antares-broker (what cargo auditable embeds):"
    printf '%s\n' "$found"
    return 1
  fi
  echo "vanilla metadata: OK ($(printf '%s\n' "$packages" | wc -l) packages, none a surface)"
}

if [ "${1:-}" = --metadata ]; then
  metadata
  exit
fi

if [ "${1:-}" = --audit-info ]; then
  bin=${2:?usage: --audit-info <binary>}
  packages=$(rust-audit-info "$bin" | jq -r '.packages[].name')
  printf '%s\n' "$packages" | grep -qx antares-broker || {
    echo "no antares-broker in the embedded package list of $bin: was it built with cargo auditable?"
    exit 1
  }
  if found=$(printf '%s\n' "$packages" | grep -E "$PACKAGES"); then
    echo "SURFACE IN THE SHIPPED BINARY $bin:"
    printf '%s\n' "$found"
    exit 1
  fi
  echo "vanilla audit-info: OK ($(printf '%s\n' "$packages" | wc -l) packages, none a surface)"
  exit 0
fi

metadata

# Wherever cargo puts it (CARGO_TARGET_DIR included).
bin="$(cargo metadata --locked --format-version 1 --no-deps | jq -r .target_directory)/debug/antares"
symbols() { nm --defined-only --demangle "$1" | grep -E "$SYMBOLS|$JC_SYMBOLS" || true; }
scratch=$(mktemp -d)
trap 'rm -rf "${scratch:?}"' EXIT

# Self-test first, so the binary left behind is the vanilla one.
cargo build --locked -p antares-broker --features plugin-example,jc
cp "$bin" "$scratch/addon"
# a here-string, not a pipe: grep -q stops early and pipefail would read nm's SIGPIPE as a miss
addon=$(symbols "$scratch/addon")
grep -Evq "$JC_SYMBOLS" <<<"$addon" || {
  echo "self-test failed: the pattern finds no symbol in a build that DOES link plugin-example"
  exit 1
}
grep -Eq "$JC_SYMBOLS" <<<"$addon" || {
  echo "self-test failed: the jc pattern finds no symbol in a build that DOES carry jc"
  exit 1
}

cargo build --locked -p antares-broker
total=$(nm --defined-only "$bin" | wc -l)
[ "$total" -gt 10000 ] || { echo "nm read $total symbols: the gate would pass vacuously"; exit 1; }
found=$(symbols "$bin")
if [ -n "$found" ]; then
  echo "SURFACE OR PLATFORM (jc) CODE LINKED INTO THE VANILLA BINARY:"
  printf '%s\n' "$found" | head -40
  exit 1
fi

# The route half at startup: a name off the shelf is fatal, never ignored.
set +e
out=$(ANTARES_API_SURFACES=admin,odata timeout 60 "$bin" 2>&1)
code=$?
set -e
if [ "$code" -eq 0 ] || [ "$code" -eq 124 ] || ! grep -q "unknown surface" <<<"$out"; then
  echo "the vanilla binary did not refuse ANTARES_API_SURFACES=admin,odata (exit $code):"
  printf '%s\n' "$out" | tail -20
  exit 1
fi
echo "vanilla symbols: OK ($total symbols, none a surface; admin,odata refused)"
