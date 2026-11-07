#!/bin/sh
# The build lane's own check of AP-142 (T-3327), run inside the app-build-rust image by the image
# lane: the SDK's React + Rust/WebAssembly example, mounted read-only at /src, built as build-app
# builds a ui App with wasm/ (crate, interface tests, typecheck, bundle, browser checks), offline.
set -eu
cp -r /src /tmp/app
cd /tmp/app
build-wasm /tmp/app /tmp/work
mkdir node_modules
for entry in /opt/template/node_modules/* /opt/template/node_modules/.bin /opt/template/node_modules/.pnpm; do
  ln -s "$entry" node_modules/
done
node /opt/template/lane.mjs deps .
TESTS=$(node /opt/template/lane.mjs vitest-config .)
node_modules/.bin/vitest run --config "$TESTS"
node_modules/.bin/tsc -b
node_modules/.bin/vite build --outDir /tmp/out --emptyOutDir
ls /tmp/out/assets/*.wasm
node /opt/template/lane.mjs browser-checks /tmp/app /tmp/out /tmp/report
