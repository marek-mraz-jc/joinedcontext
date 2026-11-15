#!/usr/bin/env bash
# Every hostname the edge serves for <domain>, one per line: the apex and each `subDomain` of the
# components' APISIX routes, the same list the edge Ingress and its certificate are rendered from
# (components/apisix/values/apisix/base-values.yaml.gotmpl). The upgrade job points these at the
# k3d edge in /etc/hosts, so scripts/smoke.sh reaches every host it checks (ci-full 39347776680).
#
#   scripts/k3d-edge-hosts.sh joinedcontext.test
set -euo pipefail

domain="${1:?usage: k3d-edge-hosts.sh <domain>}"
case "$domain" in
*[!a-z0-9.-]* | .* | *.) echo "k3d-edge-hosts: not a domain: $domain" >&2; exit 2 ;;
esac
cd "$(dirname "$0")/.."
{
	echo "$domain"
	grep -hoE '^[[:space:]]*subDomain:[[:space:]]*[a-z0-9-]+[[:space:]]*$' components/*/apisix-routes.yaml* |
		awk -v d="$domain" '{ print $2 "." d }'
} | sort -u
