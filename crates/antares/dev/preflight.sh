#!/usr/bin/env bash
# SPDX-License-Identifier: EUPL-1.2
# Every CI-only check of the crates a branch touches, in one local run before
# the push (T-3763): a train then needs one CI cycle, not one per red check.
# No coverage: the 100 % gate left preflight and CI (owner 2026-10-30,
# T-6147); dev/coverage-gate.py still measures a crate by hand. Since T-6151
# (owner 2026-10-30, push to dev under 5 minutes) it mirrors the per-push
# gate only (workspace.yml scope `push`): fmt, clippy, the tests of the
# touched crates, spdx and the repository rules that need no build. The
# vanilla symbol gate, config keys, fuzz and new-surface builds, SBOM, docs
# and each new test alone run in the weekly sweep (sweep.yml).
# Each step prints `PASS|FAIL <step> <seconds>s`; a failed step prints the
# tail of its output and the run goes on. A tool that is missing is FAIL with
# the reason, never a PASS. Exit 1 when a step failed.
#
#   dev/preflight.sh [--base REF] [--log FILE]
#   dev/preflight.sh --self-test       one deliberate break per step class
#
# The touched crates are those of `git diff --name-only $(merge-base REF HEAD)`
# plus untracked files (default REF origin/dev); antares-api brings its
# reverse dependent joinedcontext-policy (train 4). With --log the output also
# goes to FILE, so an agent can start it with run_in_background and read FILE.
#
# Each step names the CI job it mirrors. A new CI-only check lands here in
# the same change (CONTRIBUTING.md, "Before the push").
set -uo pipefail
self=$(cd "$(dirname "$0")" && pwd)/preflight.sh
# the checkout the caller stands in (the self-test runs this file in a scratch worktree)
cd "$(git rev-parse --show-toplevel)" || exit 2
root=$PWD

base=origin/dev log="" selftest=0
while (($#)); do
  case $1 in
    --base) base=${2:?--base needs a ref}; shift 2 ;;
    --no-coverage) shift ;; # accepted for old callers; nothing to skip
    --log) log=${2:?--log needs a file}; shift 2 ;;
    --self-test) selftest=1; shift ;;
    *) echo "usage: dev/preflight.sh [--base REF] [--log FILE] | --self-test" >&2; exit 2 ;;
  esac
done
[[ -n $log ]] && exec > >(tee "$log") 2>&1

# --self-test: a scratch worktree of HEAD, one commit per break class on
# odata-axum (small, role protocol, outside the vanilla tree), and the steps
# each run must turn red, exactly those
self_test() {
  export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target} # the warm build
  local tmp wt head lib=crates/odata-axum/src/lib.rs bad=0
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/preflight-self.XXXXXX")
  wt=$tmp/wt head=$(git rev-parse HEAD)
  git worktree add -q --detach "$wt" "$head" || return 2
  # shellcheck disable=SC2064 # the paths are fixed now
  trap "git -C '$wt' reset -q --hard; git -C '$root' worktree remove '$wt'; rm -rf '${tmp:?}'" EXIT
  probe() { # <case> <expected red steps, | separated> <preflight flags> <break script>
    git -C "$wt" reset -q --hard "$head"
    if [[ -n $4 ]]; then
      (cd "$wt" && bash -c "$4" && git add -A && git -c user.name=preflight -c user.email=preflight@localhost commit -qm "probe $1") || {
        echo "self-test $1: the break did not apply"; bad=1; return; }
    fi
    local run got
    # shellcheck disable=SC2086 # $3 is a flag list
    # a hang is a red run, never a self-test that waits forever
    run=$(cd "$wt" && timeout 3600 "$self" --base HEAD~"$([[ -n $4 ]] && echo 1 || echo 0)" $3 2>&1)
    got=$(sed -n 's/^FAIL \(.*\) [0-9]*s$/\1/p' <<< "$run" | paste -sd'|')
    if ! grep -q '^preflight: [0-9]* steps' <<< "$run"; then
      echo "FAIL $1: the run did not finish"; sed 's/^/    /' <<< "$run" | tail -n 20; bad=1
    elif [[ $got == "$2" ]]; then echo "ok   $1: red ${2:-nothing}"; else echo "FAIL $1: red '$got', expected '${2}'"; bad=1; fi
  }
  # with --log as the train runs it: the final wait must not wait for its tee
  probe clean "" "--log $tmp/clean.log" ""
  grep -q '^preflight: [0-9]* steps' "$tmp/clean.log" || { echo "FAIL clean: --log holds no verdict"; bad=1; }
  # a lane killed mid-step: that step and the lane read red, the run still ends with a verdict
  PREFLIGHT_DIE_AT="test" probe dead-lane "test|lane debug_lane" "" "echo '// probe' >> $lib"
  probe fmt fmt "" "sed -i '0,/^use /s//use  /' $lib"
  probe unused-dependency machete "" \
    "sed -i '/^axum = /a uuid = { version = \"1\", default-features = false }' crates/odata-axum/Cargo.toml && cargo metadata --format-version 1 > /dev/null"
  probe architecture-row spec "" "mkdir -p crates/zz-probe/src && printf '// SPDX-License-Identifier: EUPL-1.2\n//! probe\n' > crates/zz-probe/src/lib.rs"
  probe licence-line spdx "" "echo '//! probe' > crates/odata-axum/src/preflight_probe.rs"
  return "$bad"
}
if ((selftest)); then
  self_test
  exit
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/preflight.XXXXXX")
mkdir -p "$work/res" "$work/done"
start=$SECONDS

# step <key> <name> <command...>: run, time, write one line to res/<key>; on
# FAIL the tail too. The steps run side by side (T-4032), and the keys sort in
# the serial order, so the report reads as it always did, each tail under its
# own FAIL line. The FAIL goes in first and the verdict overwrites it, so a
# step whose lane is killed under it reads red, never as a missing line.
step() {
  local key=$1 name=$2 at=$SECONDS out
  shift 2
  out="$work/$key-${name//[^A-Za-z0-9_.-]/_}.txt"
  { echo "FAIL $name 0s"; echo "    did not finish: its lane ended before the verdict"; } > "$work/res/$key"
  # the self-test's lane that dies mid-step
  [[ ${PREFLIGHT_DIE_AT:-} == "$name" ]] && kill -KILL "$BASHPID"
  if "$@" > "$out" 2>&1 < /dev/null; then
    echo "PASS $name $((SECONDS - at))s" > "$work/res/$key"
  else
    { echo "FAIL $name $((SECONDS - at))s"; tail -n 25 "$out" | sed 's/^/    /'; } > "$work/res/$key"
  fi
}

# Cargo lanes in the background, at most `slots` at a time: one below 6 jobs,
# so a small worker builds one thing at once as before, else one per 3 jobs, at
# most 3. Each lane builds in a profile or target directory of its own (debug,
# release, llvm-cov, fuzz), so no lane waits on another's lock. The steps
# without a build run beside them unbounded. Each cargo lane gets its share of
# the jobs, never all of them: on a shared host the limit is memory during
# linking, and three lanes of full CARGO_BUILD_JOBS link three times at once.
total=${CARGO_BUILD_JOBS:-$(nproc)}
slots=$((total < 6 ? 1 : total / 3 < 3 ? total / 3 : 3))
share=$((total / slots > 1 ? total / slots : 1))
lanes=() lane_of=()
lane() { # <function> [args...]
  local p live
  while :; do
    live=()
    for p in "${lanes[@]}"; do kill -0 "$p" 2> /dev/null && live+=("$p"); done
    ((${#live[@]} < slots)) && break
    wait -n "${live[@]}"
  done
  bg -j "$share" "$@"
}
# bg [-j JOBS] <function> [args...]: in the background; a lane that ends
# before its last command (killed, out of memory, a shell error) leaves no
# done marker and reads red below
bg() {
  if [[ $1 == -j ]]; then
    { export CARGO_BUILD_JOBS=$2; shift 2; "$@"; touch "$work/done/$BASHPID"; } &
  else
    { "$@"; touch "$work/done/$BASHPID"; } &
  fi
  lanes+=($!)
  lane_of[$!]=$([[ $1 == -j ]] && shift 2; [[ $1 == step ]] && echo "$3" || echo "$1")
}

mb=$(git merge-base "$base" HEAD) || { echo "preflight: no merge base with $base" >&2; exit 2; }
changed=$({ git diff --name-only "$mb"; git ls-files --others --exclude-standard; } | sort -u)

# touched workspace crates, longest manifest dir first; antares-api adds its
# reverse dependent addon
meta=$(cargo metadata --locked --no-deps --format-version 1) || { echo "preflight: cargo metadata failed" >&2; exit 2; }
mapfile -t crates < <(python3 -c '
import json, os, sys
root, changed = sys.argv[1], sys.argv[2].split()
pkgs = sorted(((os.path.relpath(os.path.dirname(p["manifest_path"]), root), p["name"])
               for p in json.load(sys.stdin)["packages"]), key=lambda d: -len(d[0]))
hit = {next((n for d, n in pkgs if d != "." and f.startswith(d + "/")), None) for f in changed} - {None}
if "antares-api" in hit:
    hit.add("joinedcontext-policy")
print("\n".join(sorted(hit)))
' "$root" "$changed" <<< "$meta" | grep .)
pflags=()
for c in "${crates[@]}"; do pflags+=(-p "$c"); done
echo "preflight: base $base (${mb:0:7}), crates: ${crates[*]:-none}"

# workspace.yml's postgis service, for the crates whose pg tests skip without
# ANTARES_TEST_DATABASE_URL: the same image (PostgreSQL 16, the major dev and
# prod run, T-6021) on a port docker picks, removed when the run ends
pg_name=preflight-pg-$$
postgis() {
  local image port i
  image=$(sed -n 's/^ *image: \(postgis\/postgis:.*\)$/\1/p' .github/workflows/workspace.yml | head -1)
  [[ -n $image ]] || { echo "no postgis/postgis image line in .github/workflows/workspace.yml"; return 1; }
  docker info > /dev/null 2>&1 || {
    echo "missing tool: a running docker (the pg tests of ${crates[*]} skip without a database); or set ANTARES_TEST_DATABASE_URL"
    return 1
  }
  docker run -d --rm --name "$pg_name" --shm-size=1g -p 127.0.0.1::5432 \
    -e POSTGRES_USER=antares -e POSTGRES_PASSWORD=antares -e POSTGRES_DB=antares "$image" || return 1
  # over TCP: the entrypoint's init server listens on the socket only
  for ((i = 0; i < 120; i++)); do
    docker exec "$pg_name" pg_isready -q -h 127.0.0.1 -U antares && break
    sleep 1
  done
  ((i < 120)) || { docker logs "$pg_name" | tail -n 20; return 1; }
  port=$(docker port "$pg_name" 5432/tcp | sed -n '1s/.*://p')
  docker exec "$pg_name" psql -U antares -Atc 'SELECT version(), postgis_lib_version()'
  echo "postgresql://antares:antares@localhost:$port/antares" > "$work/pg-url"
}
if [[ -z ${ANTARES_TEST_DATABASE_URL:-} ]] && printf '%s\n' "${crates[@]}" | grep -qxE 'antares-(sql|api|broker|ops)'; then
  trap 'docker rm -f "$pg_name" > /dev/null 2>&1' EXIT
  step 00 postgis postgis
  [[ -s $work/pg-url ]] && ANTARES_TEST_DATABASE_URL=$(< "$work/pg-url") && export ANTARES_TEST_DATABASE_URL
fi

# the debug lane: clippy and the tests of the touched crates
debug_lane() {
  if ((${#crates[@]})); then
    # workspace.yml "cargo clippy" (the touched crates of its --workspace wall)
    step 02 clippy cargo clippy --locked "${pflags[@]}" --all-targets --all-features -- -D warnings
    # workspace.yml "dev/unit-junit.sh all-features"
    step 03 test cargo test --locked "${pflags[@]}" --all-features
    # strict.yml "cargo doc --document-private-items" (daily, T-6327): a doc
    # link from a pub item to a private or cfg(test) one fails only there
    step 04 doc env RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --document-private-items "${pflags[@]}"
  else
    echo "PASS clippy 0s (no crate touched)" > "$work/res/02"
    echo "PASS test 0s (no crate touched)" > "$work/res/03"
    echo "PASS doc 0s (no crate touched)" > "$work/res/04"
  fi
}
lane debug_lane
# workspace.yml "cargo fmt --all --check"
bg step 01 fmt cargo fmt --all --check
# workspace.yml "cargo-machete"
bg step 05 machete bash -c 'need() { "$@" >/dev/null 2>&1 || { echo "missing tool: $*"; exit 1; }; }; need cargo machete --version && cargo machete'
# workspace.yml "conformance ledger check"
bg step 07 spec bash -c 'python3 dev/spec.py check && python3 dev/spec_test.py'
# workspace.yml "Module rules"
bg step 08 module-rules python3 dev/check-module-rules.py
bg step 08b module-graph python3 dev/module-graph.py
# workspace.yml "Vanilla feature tree is committed"
bg step 09 vanilla-tree bash -c 'set -o pipefail; dev/vanilla-tree.sh | diff -u ci/vanilla-tree.txt -'
# workspace.yml "Licence headers"
bg step 13 spdx dev/spdx-check.sh
# workspace.yml "Route docs check" and "Env-var docs check"
bg step 14 route-docs dev/check-route-docs.sh
bg step 15 env-docs dev/check-env-docs.sh
# workspace.yml "Migration rollback manifest"
bg step 16 migrations bash -c 'python3 dev/check-migrations.py --self-test && python3 dev/check-migrations.py'
# workspace.yml "Coverage passes run to the end"
bg step 17 etsi-coverage dev/etsi-coverage.test.sh
# Our own pids only: a bare `wait` also waits for the `tee` behind --log
# (bash 5.1+), which never exits while this shell holds its stdout.
wait "${lanes[@]}"
k=0
for p in "${lanes[@]}"; do
  [[ -e $work/done/$p ]] && continue
  k=$((k + 1))
  { echo "FAIL lane ${lane_of[$p]} 0s"; echo "    the lane ended before its last step: a step it never reached has no line"; } > "$work/res/99-$(printf %02d "$k")"
done

cat "$work"/res/*
steps=$(cat "$work"/res/* | grep -c '^\(PASS\|FAIL\) ')
red=$(cat "$work"/res/* | grep -c '^FAIL ')
echo "preflight: $steps steps, $red red, $((SECONDS - start)) s (step output in $work)"
((red == 0))
