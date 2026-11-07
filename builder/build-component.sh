#!/bin/sh
# The server component of a `wasm` App (AP-151, ADR-N-044): its crate in `server/`, tested and
# compiled to wasm32-wasip2 offline against the runner's crate store, checked to be a component
# that exports `wasi:http/incoming-handler`, and copied to `<out>/.jc/component.wasm`, where the
# bundle carries it. Run by build-app on the app-build-rust runner, as the App's own code.
#
#   build-component <app-dir> <work-dir> <bundle-dir>
#
# JC_CRATE_STORE is the crate store's registry folder (default the runner's /opt/cargo/registry);
# the git checkouts beside it (`../git`) hold the guest SDK, `jc-app-sdk`, at the commits the
# reference Apps pin (T-3346).
set -eu

fail() { echo "build failed: $*" >&2; exit 1; }
APP=${1:?usage: build-component <app-dir> <work-dir> <bundle-dir>}
WORK=${2:?usage: build-component <app-dir> <work-dir> <bundle-dir>}
OUT=${3:?usage: build-component <app-dir> <work-dir> <bundle-dir>}
STORE=${JC_CRATE_STORE:-/opt/cargo/registry}
CRATE=$APP/server

[ -f "$CRATE/Cargo.toml" ] || fail "server/ holds no Cargo.toml"
[ -f "$CRATE/Cargo.lock" ] || fail "server/ holds no Cargo.lock: the crates are built --locked (AP-151)"
[ -d "$STORE/index" ] && [ -d "$STORE/cache" ] || fail "this runner carries no crate store: a wasm App builds on app-build-rust (AP-151)"

# Cargo's home is this job's, the store's index and .crate files linked read-only: nothing is
# fetched and no build leaves the store changed (as build-wasm does, AP-142).
export CARGO_HOME="$WORK/component-cargo" CARGO_TARGET_DIR="$WORK/component-target" CARGO_NET_OFFLINE=true
rm -rf "${CARGO_HOME:?}" "${CARGO_TARGET_DIR:?}"
mkdir -p "$CARGO_HOME/registry"
ln -s "$STORE/index" "$STORE/cache" "$CARGO_HOME/registry/"
[ -d "$STORE/../git" ] && ln -s "$STORE/../git" "$CARGO_HOME/git"
node "${JC_LANE:-/opt/template}/lane.mjs" server-check "$APP" || fail "server/ reaches for what a component never has (AP-147)"
cd "$CRATE"
echo "== server tests"
cargo test --offline --locked || fail "cargo test failed in server/ (a crate outside the runner's store fails here too)"
echo "== server component"
cargo build --release --offline --locked --target wasm32-wasip2 || fail "cargo build --target wasm32-wasip2 failed in server/"
NAME=$(cargo metadata --offline --no-deps --format-version 1 | node -e '
  const meta = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const root = meta.packages.find((p) => p.manifest_path === process.argv[1]);
  const libs = (root?.targets ?? []).filter((t) => t.kind.includes("cdylib"));
  if (libs.length !== 1) { console.error("server/Cargo.toml names no cdylib: crate-type = [\"cdylib\"]"); process.exit(1); }
  console.log(libs[0].name.replace(/-/g, "_"));' "$CRATE/Cargo.toml") || fail "cannot tell which library is the component"
BUILT=$CARGO_TARGET_DIR/wasm32-wasip2/release/$NAME.wasm
[ -f "$BUILT" ] || fail "cargo built no $NAME.wasm"
node "${JC_LANE:-/opt/template}/lane.mjs" component-check "$BUILT" \
  || fail "server/ built $NAME.wasm, which is not a component exporting wasi:http/incoming-handler (AP-143)"
mkdir -p "$OUT/.jc"
cp "$BUILT" "$OUT/.jc/component.wasm"
echo ".jc/component.wasm: $(wc -c < "$OUT/.jc/component.wasm") bytes, sha256:$(sha256sum "$OUT/.jc/component.wasm" | cut -d' ' -f1)"
