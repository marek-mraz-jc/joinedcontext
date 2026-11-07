#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Exit 0 when every path on stdin is a file tasks/train writes from a train's
# own surfaces-conformance run (T-6046): fast.yml then counts a dev push of
# only those files on top of a green sha as proven. Any other path, or none, exits 1.
# --selftest checks the rule.
set -euo pipefail
if [ "${1:-}" = --selftest ]; then
  self=$0
  ok() { printf '%s\n' "$@" | "$self" || { echo "selftest: expected proven: $*" >&2; exit 1; }; }
  no() { ! printf '%s\n' "$@" | "$self" || { echo "selftest: expected a run: $*" >&2; exit 1; }; }
  ok docs/src/odata-compliance.md
  ok ci/odata-compliance-baseline.json docs/src/odata-compliance.md
  no docs/src/odata-compliance.md crates/antares-odata/src/lib.rs
  no ci/odata-compliance-baseline.json.orig
  no docs/src/odata-compliance.md.bak
  no ""
  echo "suite-written selftest: ok"
  exit 0
fi
seen=0
while IFS= read -r f; do
  case $f in
    ci/odata-compliance-baseline.json | docs/src/odata-compliance.md) seen=1 ;;
    *) exit 1 ;;
  esac
done
[ "$seen" = 1 ]
