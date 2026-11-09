#!/usr/bin/env sh
# Every Rust source file opens with the SPDX licence line; list the ones that
# do not and fail. `dev/spdx-check.sh fix` prepends the line instead.
# The shared OData crates are MIT (owner 2026-11-08, T-6642); the rest EUPL-1.2.
# POSIX sh has no pipefail; the one pipeline below is guarded by hand.
set -eu
cd "$(dirname "$0")/.."
EUPL='// SPDX-License-Identifier: EUPL-1.2'
MIT='// SPDX-License-Identifier: MIT'
line_for() {
  case "$1" in
    crates/odata-url/*|crates/odata-csdl/*|crates/odata-json/*|crates/odata-batch/*|crates/odata-axum/*|crates/odata-table/*) echo "$MIT" ;;
    *) echo "$EUPL" ;;
  esac
}
missing=$(find crates -name '*.rs' -not -path '*/target/*' | while read -r f; do
  [ "$(head -n1 "$f")" = "$(line_for "$f")" ] || echo "$f"
done)
[ -n "$missing" ] || { echo "spdx: every source file carries the licence line"; exit 0; }
if [ "${1:-}" = fix ]; then
  echo "$missing" | while read -r f; do line_for "$f" | cat - "$f" > "$f.spdx" && mv "$f.spdx" "$f"; done
  echo "spdx: header added to $(echo "$missing" | wc -l) files"
else
  echo "$missing"; echo "spdx: files above lack their licence line (MIT in crates/odata-{url,csdl,json,batch,axum,table}, else EUPL-1.2; run dev/spdx-check.sh fix)"; exit 1
fi
