#!/bin/sh
# The WebAssembly part of a `ui` App (AP-142, T-3327): its crate in `wasm/`, tested and compiled to
# wasm32-unknown-unknown offline against the runner's crate store, then bound with
# `wasm-bindgen --target web` into `wasm/pkg/`, which the interface imports before `vite build`.
# Run by build-app on the app-build-rust runner, as the App's own code (no token in its env).
#
#   build-wasm <app-dir> <work-dir>
#
# JC_CRATE_STORE is the crate store's registry folder (default the runner's /opt/cargo/registry).
set -eu

fail() { echo "build failed: $*" >&2; exit 1; }
APP=${1:?usage: build-wasm <app-dir> <work-dir>}
WORK=${2:?usage: build-wasm <app-dir> <work-dir>}
STORE=${JC_CRATE_STORE:-/opt/cargo/registry}
CRATE=$APP/wasm

[ -f "$CRATE/Cargo.toml" ] || fail "wasm/ holds no Cargo.toml"
[ -f "$CRATE/Cargo.lock" ] || fail "wasm/ holds no Cargo.lock: the crates are built --locked (AP-142)"
[ -d "$STORE/index" ] && [ -d "$STORE/cache" ] || fail "this runner carries no crate store: a ui App with wasm/ builds on app-build-rust (AP-142)"
command -v wasm-bindgen >/dev/null || fail "this runner carries no wasm-bindgen (AP-106)"

# The bindings must be written by the wasm-bindgen the crate was compiled against: the format
# between the two is not stable, and a mismatch is a module that fails in the browser.
WANT=$(grep -A1 '^name = "wasm-bindgen"$' "$CRATE/Cargo.lock" | sed -n 's/^version = "\(.*\)"$/\1/p')
HAVE=$(wasm-bindgen --version | cut -d' ' -f2)
[ -n "$WANT" ] || fail "wasm/Cargo.lock names no wasm-bindgen"
[ "$WANT" = "$HAVE" ] || fail "wasm/Cargo.lock names wasm-bindgen $WANT and this runner binds with $HAVE: pin wasm-bindgen = \"=$HAVE\" (AP-142)"

# Cargo's home is this job's, the store's index and .crate files linked read-only, as for a
# ui-rust App (build-app): nothing is fetched and no build leaves the store changed.
export CARGO_HOME="$WORK/wasm-cargo" CARGO_TARGET_DIR="$WORK/wasm-target" CARGO_NET_OFFLINE=true
rm -rf "${CARGO_HOME:?}" "${CARGO_TARGET_DIR:?}" "${CRATE:?}/pkg"
mkdir -p "$CARGO_HOME/registry"
ln -s "$STORE/index" "$STORE/cache" "$CARGO_HOME/registry/"
cd "$CRATE"
echo "== WebAssembly tests"
cargo test --offline --locked || fail "cargo test failed in wasm/ (a crate outside the runner's store fails here too)"
echo "== WebAssembly"
cargo build --release --offline --locked --target wasm32-unknown-unknown || fail "cargo build --target wasm32-unknown-unknown failed"
NAME=$(cargo metadata --offline --no-deps --format-version 1 | node -e '
  const meta = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const root = meta.packages.find((p) => p.manifest_path === process.argv[1]);
  const libs = (root?.targets ?? []).filter((t) => t.kind.includes("cdylib"));
  if (libs.length !== 1) { console.error("wasm/Cargo.toml names no cdylib: crate-type = [\"cdylib\"]"); process.exit(1); }
  console.log(libs[0].name.replace(/-/g, "_"));' "$CRATE/Cargo.toml") || fail "cannot tell which library is the module"
wasm-bindgen --target web --out-dir "$CRATE/pkg" "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/$NAME.wasm" \
  || fail "wasm-bindgen could not bind $NAME.wasm"
echo "wasm/pkg/${NAME}_bg.wasm: $(gzip -c "$CRATE/pkg/${NAME}_bg.wasm" | wc -c) bytes gzipped"
