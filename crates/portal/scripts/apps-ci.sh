#!/bin/sh
# The Apps' builds, tests and coverage gate (T-3373), from the repository root:
#
#   sh scripts/apps-ci.sh wasm   # the react-rust-wasm example and the Apps with a wasm/ crate
#   sh scripts/apps-ci.sh ui     # the reference apps/*/ui, the Apps in their own repository, the
#                                # rust-wasm-server example's interface
#   sh scripts/apps-ci.sh typecheck   # only the type check of each touched apps/*/ui (T-6341)
#   sh scripts/apps-ci.sh touched <path>   # exit 0 when the change touches <path>
#   sh scripts/apps-ci.sh ours <path>      # touched, and in this job's shard
#
# The fast lane (ci.yml) runs only what a change touched: an App when a file under apps/<name>/
# changed, everything when the SDK, the build lane's builder/ or the gate itself changed, or when
# the change cannot be told (no base). `APPS_ALL=1` runs everything (ci-full). The base is the
# branch's fork from main, or the commit before on main; `APPS_BASE` names another.
# `APPS_SHARD=i/n` splits the ui mode over n jobs: every App lands in exactly one, chosen by a
# checksum of its path, so a change to the SDK fits the job's timeout however many Apps there are.
set -eu

# An App without a lockfile resolves its dependencies when it installs. A release younger than a
# day is not taken: a half-published one (@loaders.gl/core 4.5.4 named a worker-utils 4.5.4 the
# registry did not have, 2026-10-10) cannot turn the lane red, and a malicious one has a day to be
# pulled first (T-3527). Locked installs are untouched.
export npm_config_minimum_release_age="${npm_config_minimum_release_age:-1440}"

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN="$ROOT/scripts/app-coverage-run.sh"
cd "$ROOT"

# Files whose change can move every App's result.
EVERYTHING='^(sdk/|builder/|scripts/app-coverage-|scripts/apps-ci\.sh$|apps/coverage-thresholds\.json$|\.github/workflows/ci\.yml$)'

changed_files() {
  base=${APPS_BASE:-}
  if [ -z "$base" ]; then
    git fetch -q --depth=100 origin main 2>/dev/null || true
    base=$(git merge-base HEAD origin/main 2>/dev/null || true)
    [ "$base" = "$(git rev-parse HEAD)" ] && base=$(git rev-parse -q --verify HEAD~1 2>/dev/null || true)
  fi
  # No base to compare with: say so, and everything runs.
  [ -n "$base" ] || { echo "*"; return; }
  # A base the shallow clone does not hold is no base either.
  git diff --name-only "$base" HEAD 2>/dev/null || echo "*"
}

CHANGED=""
touched() {
  [ "${APPS_ALL:-}" = "1" ] && return 0
  [ -n "$CHANGED" ] || CHANGED=$(changed_files)
  [ "$CHANGED" = "*" ] && return 0
  printf '%s\n' "$CHANGED" | grep -Eq "$EVERYTHING" && return 0
  printf '%s\n' "$CHANGED" | grep -q "^${1%/}/"
}

skip() {
  echo "apps-ci: $1 untouched by this change, left to ci-full"
}

# Touched, and this job's to run (APPS_SHARD); says why when it is not.
ours() {
  touched "$1" || { skip "$1"; return 1; }
  [ -n "${APPS_SHARD:-}" ] || return 0
  i=${APPS_SHARD%/*} n=${APPS_SHARD#*/}
  [ $(($(printf '%s' "${1%/}" | cksum | cut -d' ' -f1) % n + 1)) -eq "$i" ] && return 0
  echo "apps-ci: $1 runs in another shard than $APPS_SHARD"
  return 1
}

wasm() {
  if ours sdk/examples/react-rust-wasm; then
    work=$(mktemp -d)
    cp -r sdk/examples/react-rust-wasm/. "$work"
    cargo fetch --locked --manifest-path "$work/wasm/Cargo.toml"
    JC_CRATE_STORE="$HOME/.cargo/registry" sh builder/build-wasm.sh "$work" "$(mktemp -d)"
    (cd "$work" && npm pkg set "dependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile \
      && sh "$RUN" react-rust-wasm --rust && pnpm build \
      && pnpm exec playwright install --with-deps --only-shell chromium \
      && pnpm exec playwright test)
  fi
  for app in apps/*/; do
    [ -f "$app/wasm/Cargo.toml" ] && [ -f "$app/package.json" ] || continue
    ours "$app" || continue
    work=$(mktemp -d)
    cp -r "$app". "$work"
    cargo fetch --locked --manifest-path "$work/wasm/Cargo.toml"
    JC_CRATE_STORE="$HOME/.cargo/registry" sh builder/build-wasm.sh "$work" "$(mktemp -d)"
    # A `wasm` App's server component (AP-151): its tests, then the component the lane would pack.
    if [ -f "$work/server/Cargo.toml" ]; then
      (cd "$work/server" && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings \
        && cargo build --release --locked --target wasm32-wasip2 \
        && node "$ROOT/builder/lane.mjs" component-check \
          "$(cargo metadata --no-deps --format-version 1 | node -p 'JSON.parse(require("fs").readFileSync(0,"utf8")).target_directory')/wasm32-wasip2/release/$(basename "$app" | tr - _)_server.wasm")
    fi
    (cd "$work" && npm pkg set "dependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile && pnpm typecheck \
      && sh "$RUN" "$(basename "$app")" --rust && pnpm build \
      && pnpm exec playwright install --with-deps --only-shell chromium \
      && pnpm exec playwright test)
  done
}

# The reference apps' type check alone, seconds an App: the per-push lane builds no App since
# T-6153, so an SDK type an App fills (a new FilterOp label) broke only the image build (T-6341).
typecheck() {
  for app in apps/*/ui; do
    [ -f "$app/package.json" ] || continue
    touched "$(dirname "$app")" || { skip "$(dirname "$app")"; continue; }
    (cd "$app" && if [ -f pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; else
      npm pkg set "dependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile --config.lockfile=false; fi && pnpm typecheck)
  done
  git checkout -- apps && git clean -fq -- apps
}

ui() {
  # The reference apps of AP-34 carry their own bundle and no lockfile: the SDK name is pointed at
  # this checkout, and the manifests are put back after (the lane test reads them). An App without
  # a lockfile installs with none written, so a failed run leaves no stray lockfile the next run
  # would take as the App's own (T-6341 review).
  for app in apps/*/ui; do
    [ -f "$app/package.json" ] || continue
    ours "$(dirname "$app")" || continue
    (cd "$app" && if [ -f pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; else
      npm pkg set "dependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile --config.lockfile=false; fi && pnpm typecheck \
      && sh "$RUN" "$(basename "$(dirname "$app")")" && pnpm build)
  done
  git checkout -- apps && git clean -fq -- apps
  # An App in its own forge repository sits at the root of apps/<name>/ (AP-75): tested as that
  # tree copied out, the SDK pointed at this checkout. One with a wasm/ crate is the wasm mode's.
  for app in apps/*/; do
    [ -f "$app/package.json" ] || continue
    [ -f "$app/wasm/Cargo.toml" ] && continue
    ours "$app" || continue
    work=$(mktemp -d)
    cp -r "$app". "$work"
    (cd "$work" && npm pkg set "dependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile && pnpm typecheck \
      && sh "$RUN" "$(basename "$app")" && pnpm build)
  done
  # The plain-HTML example has no build and no package manager (AP-83): its tests run in a copy
  # given a throwaway package.json with the SDK's own vitest and jsdom.
  if ours sdk/examples/plain-html-events; then
    work=$(mktemp -d)
    cp -r sdk/examples/plain-html-events/. "$work"
    vitest=$(node -p "require('$ROOT/sdk/node_modules/vitest/package.json').version")
    jsdom=$(node -p "require('$ROOT/sdk/node_modules/jsdom/package.json').version")
    (cd "$work" && cp test/vitest.config.mjs vitest.config.mjs \
      && printf '{ "name": "plain-html-events-tests", "private": true, "type": "module" }\n' > package.json \
      && pnpm add --save-dev "vitest@$vitest" "jsdom@$jsdom" "@joinedcontext/sdk@link:$ROOT/sdk" >/dev/null \
      && sh "$RUN" plain-html-events)
  fi
  if ours sdk/examples/rust-wasm-server; then
    work=$(mktemp -d)
    cp -r sdk/examples/rust-wasm-server/. "$work"
    (cd "$work" && npm pkg set "devDependencies.@joinedcontext/sdk=link:$ROOT/sdk" \
      && pnpm install --no-frozen-lockfile && sh "$RUN" rust-wasm-server && pnpm build)
  fi
}

case "${1:-}" in
  wasm) wasm ;;
  ui) ui ;;
  typecheck) typecheck ;;
  touched) touched "${2:?usage: apps-ci.sh touched <path>}" ;;
  ours) ours "${2:?usage: apps-ci.sh ours <path>}" ;;
  *) echo "usage: apps-ci.sh wasm|ui|typecheck|touched <path>|ours <path>" >&2; exit 2 ;;
esac
