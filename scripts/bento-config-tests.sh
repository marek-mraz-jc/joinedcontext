#!/bin/sh
# Copies Bento's config/test tree (licence header on YAML and proto files; Bloblang files stay verbatim
# because a header would move the line numbers their error texts name) and records what Bento 1.22.0
# prints for every config unit-test suite in tests/bento-config-tests, so
# `cargo test -p ekiben-server --test bento_config_tests` can compare `ekiben test` without the Bento binary.
#   BENTO_SRC=<bento v1.22.0 checkout> sh scripts/bento-config-tests.sh   # recopy config/test, then record
#   sh scripts/bento-config-tests.sh                                       # record only
set -eu
cd "$(dirname "$0")/.."
sh scripts/reference-bento.sh >/dev/null
T=tests/bento-config-tests

if [ -n "${BENTO_SRC:-}" ]; then
  rm -rf tests/bento-config-tests/bento
  cp "$BENTO_SRC/LICENSE" "$T/LICENSE-bento"
  (cd "$BENTO_SRC/config/test" && find . -type f | sort) | while read -r f; do
    src="$BENTO_SRC/config/test/$f"
    dst="$T/bento/${f#./}"
    mkdir -p "$(dirname "$dst")"
    case "$f" in
      *.yaml|*.yml) c="#" ;;
      *.proto) c='//' ;;
      *) cp "$src" "$dst"; continue ;;
    esac
    {
      echo "$c Copied from github.com/warpstreamlabs/bento v1.22.0 config/test/${f#./}"
      echo "$c Copyright (c) 2024-present Bento contributors, MIT License: see LICENSE-bento beside this tree."
      cat "$src"
    } > "$dst"
  done
fi

rm -rf tests/bento-config-tests/expected
# One run per suite: a target Bento cannot set up stops a directory run at that target.
find "$T" -name '*.yaml' ! -name '*_bento_test.yaml' | sort | while read -r suite; do
  set +e
  res=$(.bento/bento test "$suite" 2>/dev/null; echo "exit $?")
  set -e
  case "$res" in "No tests were found"*) continue ;; esac
  out="$T/expected/${suite#"$T"/}.out"
  mkdir -p "$(dirname "$out")"
  printf '%s\n' "$res" > "$out"
done
echo "recorded $(find "$T/expected" -name '*.out' | wc -l) suites"
