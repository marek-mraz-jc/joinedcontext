#!/usr/bin/env bash
# SPDX-License-Identifier: EUPL-1.2
# dev/etsi-coverage.sh measures every test binary even when some are red, and a
# cell without a database skips the pg tests (T-6499: run 41519646110 lost 2.6
# points when one red binary per cell cut each pass short). A stub cargo fails
# every test run and stops the script at the broker build.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT
mkdir "$tmp/target"
cat > "$tmp/cargo" <<STUB
#!/usr/bin/env bash
case "\$1 \$2" in
  "llvm-cov show-env") echo "export CARGO_LLVM_COV_TARGET_DIR=$tmp/target" ;;
  "llvm-cov clean") ;;
  test\ *) [[ " \$* " == *" --no-run "* ]] && exit 0
    echo "\$* url=\${ANTARES_TEST_DATABASE_URL-unset}" >> "$tmp/runs"; exit 101 ;;
  *) exit 1 ;;
esac
STUB
cp "$tmp/cargo" "$tmp/cargo-llvm-cov"
chmod +x "$tmp/cargo" "$tmp/cargo-llvm-cov"

PATH="$tmp:$PATH" TMPDIR="$tmp" UNIT_TESTS=1 STORE=memory \
  ANTARES_DATABASE_URL='' ANTARES_TEST_DATABASE_URL='' \
  dev/etsi-coverage.sh > "$tmp/log" 2>&1 || true

bad=0
[ "$(wc -l < "$tmp/runs")" = 2 ] || {
  echo "expected the unit and the api pass, a red one must not stop the other:"; cat "$tmp/log"; bad=1; }
if grep -v -- '--no-fail-fast' "$tmp/runs"; then
  echo "^ a pass without --no-fail-fast loses every binary after the first red one"; bad=1
fi
if grep -v 'url=unset$' "$tmp/runs"; then
  echo "^ an empty ANTARES_TEST_DATABASE_URL reached the tests: pg tests panic instead of skipping"; bad=1
fi
[ "$bad" = 0 ] && echo "etsi-coverage passes: OK"
exit "$bad"
