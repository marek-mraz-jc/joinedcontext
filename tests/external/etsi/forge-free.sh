#!/usr/bin/env bash
# forge-free.sh <suite-dir> <base-url> - the ETSI suite reads its @contexts from
# <base-url> instead of forge.etsi.org or GitHub (T-3549), so an outage of
# either never fails a run. Every raw URL of a suite file (forge or GitHub)
# becomes <base-url>/<path in the suite>; serve the suite root there, e.g.:
#   python3 -m http.server 8078 --bind 127.0.0.1 --directory <suite-dir>
# (not 8099: the IOP TPs use that port as a dead endpoint)
# The prefix alone is rewritten, so a URL split over two string literals
# (libraries/FederationReset.py) is caught as well.
# The rewrite is committed in the suite's git checkout: dev/etsi-pipeline.sh
# restores resources/variables.py with `git checkout` between steps, and that
# must restore the rewritten file. Same approach as Coraine's
# test/etsi/etsiForgeFree.sh (Apache-2.0), without its context server.
set -euo pipefail
SUITE=${1:?usage: forge-free.sh <suite-dir> <base-url>}
BASE=${2:?usage: forge-free.sh <suite-dir> <base-url>}
REMOTE='https://(forge\.etsi\.org/rep/cim/ngsi-ld-test-suite/-/raw/[^/"'"'"' ]+|raw\.githubusercontent\.com/[^/"'"'"' ]+/[^/"'"'"' ]+/[^/"'"'"' ]+/ngsi-ld-test-suite)/'
[ -d "$SUITE/resources/jsonld-contexts" ] || { echo "forge-free.sh: no resources/jsonld-contexts in $SUITE" >&2; exit 1; }
cd "$SUITE"
files=$(grep -rlIE "$REMOTE" --exclude-dir=.git --exclude-dir=.venv --exclude-dir=__pycache__ . || true)
[ -z "$files" ] || echo "$files" | xargs sed -i -E "s#$REMOTE#${BASE%/}/#g"
echo "forge-free: $(echo "$files" | grep -c . || true) files now read their @contexts from $BASE"
left=$(grep -rnIE "$REMOTE" --exclude-dir=.git --exclude-dir=.venv --exclude-dir=__pycache__ . || true)
if [ -n "$left" ]; then
  echo "forge-free.sh: remote @context references left:" >&2
  echo "$left" | head -5 >&2
  exit 1
fi
if [ -n "$files" ]; then
  git add -A . && git -c user.name=forge-free -c user.email=forge-free@localhost commit -qm "forge-free @contexts"
fi
