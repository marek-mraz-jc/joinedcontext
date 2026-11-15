#!/usr/bin/env bash
# Render one environment into a single manifest file.
#   scripts/render.sh <environment> <output-file> [extra helmfile args...]
# With --deployed, Helm test hooks (helm.sh/hook: test) are dropped: `helmfile sync`
# never applies them, so they are not part of what runs on a cluster and must not be
# judged as if they were.
set -euo pipefail

env="${1:?usage: render.sh <environment> <output-file> [--deployed] [helmfile args...]}"
out="${2:?usage: render.sh <environment> <output-file> [--deployed] [helmfile args...]}"
shift 2
# A private environment's values exist only in the JC_ENVIRONMENTS overlay that `just _dev-assemble`
# copies in (docs/Deployment/01-repository-layout.md §2): stop before helmfile renders it from defaults.
if [ -f deployment/helmfile.yaml ] && [ ! -d "deployment/environments/$env" ]; then
	echo "environment $env is private: set JC_ENVIRONMENTS to the environments/ checkout and run just _dev-assemble" >&2
	exit 1
fi

deployed_only=false
args=()
for a in "$@"; do
	if [ "$a" = "--deployed" ]; then deployed_only=true; else args+=("$a"); fi
done

# Remote charts are pulled on every render; one TCP reset from the chart host failed a
# whole ci shard (T-5183). Retry only network errors, at most three attempts, so a
# template or values error still fails at once.
transient='connection reset by peer|i/o timeout|TLS handshake timeout|unexpected EOF|Client\.Timeout exceeded|(502|503|504) (Bad Gateway|Service Unavailable|Gateway Timeout)'
errlog="$(mktemp)"
trap 'rm -f "$errlog"' EXIT
attempt=1
until helmfile -f deployment/helmfile.yaml -e "$env" template --skip-deps -q "${args[@]+"${args[@]}"}" > "$out" 2> "$errlog"; do
	cat "$errlog" >&2
	if [ "$attempt" -ge 3 ] || ! grep -qE "$transient" "$errlog"; then exit 1; fi
	echo "render.sh: network error pulling a chart, attempt $attempt of 3 failed, retrying in $((attempt * 3)) s" >&2
	sleep $((attempt * 3))
	attempt=$((attempt + 1))
done
cat "$errlog" >&2

if [ "$deployed_only" = true ]; then
	python3 - "$out" <<'PY'
import sys, yaml
path = sys.argv[1]
with open(path) as fh:
    docs = [d for d in yaml.safe_load_all(fh) if d]
def is_test_hook(doc):
    hooks = ((doc.get("metadata") or {}).get("annotations") or {}).get("helm.sh/hook", "")
    return "test" in str(hooks).split(",")

kept = [d for d in docs if isinstance(d, dict) and not is_test_hook(d)]
with open(path, "w") as fh:
    yaml.safe_dump_all(kept, fh, default_flow_style=False)
print(f"{path}: {len(kept)} objects ({len(docs) - len(kept)} helm test hooks dropped)")
PY
fi
