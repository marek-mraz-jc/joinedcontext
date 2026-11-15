#!/usr/bin/env bash
# Writes the scratch environment file a release's test-deployment-variants.sh installs variant
# 0,0,0 with, by running that harness's own write_env_overlay. The harness removes the file on
# exit, so ci-upgrade-e2e.sh rebuilds it here before upgrading from that release (T-4784).
#
#   prev-env-overlay.sh <path to the release's test-deployment-variants.sh> <file to write>

set -euo pipefail

HARNESS="${1:?usage: prev-env-overlay.sh <harness> <env file>}"
ENV_FILE="${2:?usage: prev-env-overlay.sh <harness> <env file>}"

fn="$(awk '/^write_env_overlay\(\)/,/^}/' "$HARNESS")"
profile="$(sed -n 's/^PROFILE="\([a-z]*\)".*/\1/p' "$HARNESS" | head -1)"
# componentProfile default (T-4856); a release older than it has no COMPONENTS and never reads it.
components="$(sed -n 's/^COMPONENTS="\([a-z-]*\)".*/\1/p' "$HARNESS" | head -1)"
if [ -z "$fn" ] || [ -z "$profile" ]; then
	echo "no write_env_overlay or PROFILE default in $HARNESS" >&2
	exit 1
fi

export ENV_FILE PROFILE="$profile" COMPONENTS="${components:-full}"   # read by the eval'd write_env_overlay
eval "$fn"
write_env_overlay 0 0
