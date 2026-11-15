#!/usr/bin/env bash
# Render several environments in parallel, each into <outdir>/<env>.yaml (T-5552).
#   scripts/render-all.sh <outdir> <environment>...
# RENDER_JOBS renders run at once (default 3). `helmfile repos` runs first, one
# environment after another, because it writes the shared helm repository cache.
# Each render's stderr goes to <outdir>/<env>.log. A failed render prints its log, the
# others still finish, and the script exits 1 naming every environment that failed.
set -euo pipefail

out="${1:?usage: render-all.sh <outdir> <environment>...}"
shift
[ "$#" -gt 0 ] || { echo "usage: render-all.sh <outdir> <environment>..." >&2; exit 2; }
jobs="${RENDER_JOBS:-3}"
[[ "$jobs" =~ ^[1-9][0-9]?$ ]] || { echo "render-all.sh: RENDER_JOBS must be 1-99, got '$jobs'" >&2; exit 2; }
for env in "$@"; do
	# The name becomes a file name under $out: no path separators or dot segments.
	[[ "$env" =~ ^[a-z0-9][a-z0-9-]*$ ]] || { echo "render-all.sh: bad environment name '$env'" >&2; exit 2; }
done

mkdir -p "$out"
for env in "$@"; do rm -f "$out/$env.yaml" "$out/$env.log" "$out/$env.failed"; done
for env in "$@"; do
	helmfile -f deployment/helmfile.yaml -e "$env" repos >/dev/null
done

render="$(dirname "$0")/render.sh"
started=$SECONDS
# xargs keeps going after one render fails; the .failed marker says which.
# shellcheck disable=SC2016 # $0..$2 belong to the inner bash
printf '%s\n' "$@" | xargs -P "$jobs" -I{} bash -c \
	'"$0" "$1" "$2/$1.yaml" 2>"$2/$1.log" || touch "$2/$1.failed"' "$render" {} "$out" || true

failed=()
for env in "$@"; do
	if [ -e "$out/$env.failed" ] || [ ! -e "$out/$env.yaml" ]; then
		failed+=("$env")
		echo "--- render of $env failed:" >&2
		cat "$out/$env.log" >&2 2>/dev/null || true
	fi
done
echo "render-all.sh: $# environments in $((SECONDS - started)) s, $jobs at once" >&2
if [ "${#failed[@]}" -gt 0 ]; then
	echo "render-all.sh: render failed for: ${failed[*]}" >&2
	exit 1
fi
