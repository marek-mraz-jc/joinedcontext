#!/bin/sh
# Builds jc and the kaisatsu CLI in release inside the Rust image bins/jc/Dockerfile's `rust` stage
# names (rust:<version>-slim-trixie, by digest), so the binary links the glibc of the image's
# Debian, never the runner's: ubuntu-latest became 26.04 (glibc 2.43) and a runner-built jc failed
# the trixie image smoke with "GLIBC_2.43 not found" (T-7287). The workspace and CARGO_HOME are
# mounted at their runner paths and the build runs as the runner's user, so .github/actions/rust's
# target cache (rust-cache) and sccache (GitHub Actions backend, the env that action exports) keep
# working: cargo writes target/ and the registry as on the runner, and rust-cache saves them.
#   sh build-in-base.sh [Dockerfile]
#   sh build-in-base.sh --selftest
set -eu

# base <Dockerfile>: the image of the `rust` stage; fails unless it is pinned by digest
base() {
  img=$(awk '$1 == "FROM" && tolower($3) == "as" && $4 == "rust" { print $2 }' "$1")
  printf '%s' "$img" | grep -qE '^[^@ ]+@sha256:[0-9a-f]{64}$' || {
    echo "::error file=$1::the rust stage's image '$img' is not pinned by digest" >&2
    return 1
  }
  printf '%s\n' "$img"
}

if [ "${1:-}" = --selftest ]; then
  d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
  sha=$(printf '%064d' 0 | tr 0 a)
  printf 'FROM node:24@sha256:%s AS ui\nFROM rust:1.99.0-slim-trixie@sha256:%s AS rust\nFROM rust AS build\n' "$sha" "$sha" > "$d/Dockerfile"
  [ "$(base "$d/Dockerfile")" = "rust:1.99.0-slim-trixie@sha256:$sha" ] || { echo "FAIL: pinned rust stage not read"; exit 1; }
  printf 'FROM rust:1.99.0-slim-trixie AS rust\n' > "$d/Dockerfile"
  ! base "$d/Dockerfile" 2>/dev/null || { echo "FAIL: unpinned rust stage accepted"; exit 1; }
  printf 'FROM debian:trixie-slim AS overlay\n' > "$d/Dockerfile"
  ! base "$d/Dockerfile" 2>/dev/null || { echo "FAIL: missing rust stage accepted"; exit 1; }
  echo "build-in-base.sh selftest ok"
  exit 0
fi

img=$(base "${1:-bins/jc/Dockerfile}")
ws=$(pwd)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
mkdir -p "$cargo_home/registry" "$cargo_home/git"
# The build tools the Dockerfile's rust stage installs (make for tikv-jemalloc-sys, cmake for
# aws-lc-sys, g++ for the assistant's tokenizer), in an image of its own: the build itself runs
# unprivileged.
printf 'FROM %s\nRUN apt-get update && apt-get install -y --no-install-recommends make cmake g++ && rm -rf /var/lib/apt/lists/*\n' "$img" \
  | docker build -q -t jc-build-base -

# sccache from the runner (sccache-action installs the static musl build), when the env names it.
set --
if [ "${RUSTC_WRAPPER:-}" = sccache ]; then
  set -- -v "$(command -v sccache):/usr/local/bin/sccache:ro" \
    -e RUSTC_WRAPPER -e SCCACHE_GHA_ENABLED -e SCCACHE_IDLE_TIMEOUT -e SCCACHE_GHA_RW_MODE \
    -e ACTIONS_CACHE_SERVICE_V2 -e ACTIONS_RESULTS_URL -e ACTIONS_RUNTIME_TOKEN -e ACTIONS_CACHE_URL
fi
# Secrets go by name (-e NAME), so no value is on a command line.
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$ws:$ws" -w "$ws" -v "$cargo_home/registry:$cargo_home/registry" -v "$cargo_home/git:$cargo_home/git" \
  -e CARGO_HOME="$cargo_home" -e HOME=/tmp -e CARGO_TERM_COLOR -e CARGO_INCREMENTAL -e CARGO_BUILD_JOBS "$@" \
  jc-build-base sh -euc '
    cargo --version
    cargo build --release --locked -p jc -p kaisatsu-server
    if [ -n "${RUSTC_WRAPPER:-}" ]; then sccache --show-stats; fi
    ldd --version | head -1'
