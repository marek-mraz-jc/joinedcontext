#!/usr/bin/env bash
# What the platform build costs over vanilla Antares on the same request
# shapes (T-4853, docs/src/performance.md "Vanilla against the platform
# build"): dev/perf/shapes.sh with the vanilla binary, with the platform
# image binary (`full,jc-policy,linkml,parquet,tabular,ical,feed`) as vanilla
# runs it, and with that binary under ANTARES_POLICY=joinedcontext, a
# policy document of 20 permissions and 12 Endpoint views loaded, every
# request carrying the subject the gateway would name. The runs share the
# machine, so the ratio is the number; the absolute req/s belong to perf-weekly's pinned box.
#
#   BIN_VANILLA=… BIN_PLATFORM=… dev/perf/platform-overhead.sh
#   DATABASE_URL=postgres://… …                    # + the postgres rows
#
# Env: BIN_VANILLA, BIN_PLATFORM (required), OUT (results/perf/overhead),
#      SPECS ("query 50,query-q 50,query-deep 50,query-regex 50,retrieve 50,update 50,hfp 50"), the rest as
#      shapes.sh. Synthetic Vehicles only; no credential is used.
set -euo pipefail
cd "$(dirname "$0")/../.."

# setup: the views and the rule document, against the broker on PORT
if [ "${1:-}" = setup ]; then
  base="http://127.0.0.1:${PORT:?}"
  put() {  # put <path> <json>: fails the run on a refusal
    curl -sf -o /dev/null -X PUT -H 'Content-Type: application/json' --data "$2" "$base$1" \
      || { echo "PUT $1 refused" >&2; exit 1; }
  }
  d=https://uri.etsi.org/ngsi-ld/default-context/
  attrs='"brand", "speed", "mileage", "colour", "location"'
  for i in $(seq 1 12); do
    put "/q/views/perf$i" "{\"id\": \"perf$i\", \"tenant\": \"default\", \"selector\": [\"Vehicle\"],
      \"rules\": [{\"types\": [\"Vehicle\"], \"q\": \"speed>$i\", \"attributes\": [$attrs]}],
      \"operations\": [\"query_entities\", \"retrieve_entity\"]}"
  done
  # one permission grants the caller's role the Vehicles; 19 more name
  # other roles and types, as a space with several Endpoints carries
  others=$(for i in $(seq 1 19); do
    printf ', {"id": "perf/other-%s", "assignee": {"kind": "role", "id": "endpoint:perf/e%s"},
      "operations": ["5.7.1", "5.7.2"], "information": [{"entities": [{"type": "%sType%s"}]}], "q": "level>%s"}' \
      "$i" "$i" "$d" "$i" "$i"
  done)
  put /q/policy/default "{\"policies\": [{\"id\": \"perf/vehicles\",
    \"assignee\": {\"kind\": \"role\", \"id\": \"$JC_ROLES\"},
    \"operations\": [\"5.6.2\", \"5.6.8\", \"5.6.10\", \"5.7.1\", \"5.7.2\"],
    \"information\": [{\"entities\": [{\"type\": \"${d}Vehicle\"}],
      \"propertyNames\": [\"${d}brand\", \"${d}speed\", \"${d}mileage\", \"${d}colour\",
        \"https://uri.etsi.org/ngsi-ld/location\"]}]}$others]}"
  exit 0
fi

: "${BIN_VANILLA:?vanilla binary}" "${BIN_PLATFORM:?platform binary}"
OUT="${OUT:-results/perf/overhead}"
export SPECS="${SPECS:-query 50,query-q 50,query-deep 50,query-regex 50,retrieve 50,update 50,hfp 50}"
mkdir -p "$OUT"

BIN="$BIN_VANILLA" OUT="$OUT/vanilla" dev/perf/shapes.sh >/dev/null
# the platform binary as vanilla runs it (allow-all, no view), then under
# the policy: the first difference is the build, the second the engine
BIN="$BIN_PLATFORM" OUT="$OUT/build" dev/perf/shapes.sh >/dev/null
BIN="$BIN_PLATFORM" OUT="$OUT/platform" SETUP="$0 setup" \
  ANTARES_POLICY=joinedcontext ANTARES_POLICY_SUBJECT_HEADERS=x-jc-subject,x-jc-roles \
  ANTARES_API_SURFACES=admin,views JC_SUBJECT=perf JC_ROLES=endpoint:perf/vehicles \
  dev/perf/shapes.sh >/dev/null

python3 - "$OUT"/{vanilla,build,platform}/shapes.md <<'EOF' | tee "$OUT/overhead.md"
import re, sys
row = re.compile(r"^\| (\w+) \| ([\w-]+) \| (c\d+) \| ([\d.]+) \| ([\d.]+) ms \|$")
def table(path):
    return {m.group(1, 2, 3): (float(m[4]), float(m[5])) for m in map(row.match, open(path)) if m}
van, build, plat = (table(p) for p in sys.argv[1:4])
print("| store | shape | clients | vanilla req/s, p99 | platform build | change req/s, p99 | + policy and views | change req/s, p99 |")
print("|---|---|---|---|---|---|---|---|")
for key, (vr, vp) in van.items():
    if key not in build or key not in plat:
        sys.exit(f"a platform run has no row {key}")
    cells = []
    for r, p in (build[key], plat[key]):
        cells += [f"{r:.0f}, {p:.2f} ms", f"{r / vr - 1:+.0%}, {p / vp - 1:+.0%}"]
    print(f"| {' | '.join(key)} | {vr:.0f}, {vp:.2f} ms | {' | '.join(cells)} |")
EOF
