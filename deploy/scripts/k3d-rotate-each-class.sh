#!/usr/bin/env bash
# One rotation of each in-cluster credential class, on the deployment ci-full's k3d-deploy job
# has just smoke-tested (T-2843, OPS-45; Operations/01 Runbook 5). The harness runs it with
#
#   ./scripts/test-deployment-variants.sh --after ./scripts/k3d-rotate-each-class.sh 0,0,0
#
# and smoke-tests again afterwards. What only a running cluster proves: keycloak-config-cli
# re-imports a client whose one change is its secret, CloudNativePG applies a changed role
# password, helm makes a deleted Secret again, and every reader comes back healthy.
# rotate-secret.sh measures each one: the old value refused, the new one accepted.
#
# Never against dev without an announcement in AI_shared_folder.md: a rotation restarts the
# Portal.
set -euo pipefail

env="${JC_ENV:?the harness names the scratch environment it deployed in JC_ENV}"
domain="${JC_DOMAIN:-joinedcontext.test}"
realm="${JC_REALM:-dev}"

# The edge's certificate comes from the harness's self-signed CA; curl trusts it for these
# measurements only, and nothing is ever sent with verification off.
ca="$(mktemp)"
trap 'rm -f "$ca"' EXIT
kubectl -n cert-manager get secret ca-secret -o 'jsonpath={.data.tls\.crt}' | base64 -d >"$ca"
[ -s "$ca" ] || { echo "k3d-rotate-each-class: no CA certificate in cert-manager/ca-secret" >&2; exit 1; }
export CURL_CA_BUNDLE="$ca"

rotate() { ./scripts/rotate-secret.sh --instance "$env" "$@"; }

# The Keycloak client class on a client that keeps a secret (the collector, a PF-47 exception):
# the platform's own workload clients are federated and hold none (T-2868).
rotate --secret keycloak-client-activity-ingest --idm "https://idm.${domain}" --realm "$realm"
rotate --secret db-portal
rotate --secret gitea-token-gateway --forge "https://${domain}/git"
rotate --secret portal-cookie-key
echo "k3d-rotate-each-class: one credential of each class rotated"
