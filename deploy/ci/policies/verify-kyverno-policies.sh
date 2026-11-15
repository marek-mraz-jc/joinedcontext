#!/usr/bin/env bash
set -euo pipefail

# ensure we are in the repo root
cd "$(dirname "$0")/../.."

COMPONENT=${1:-""}

selector_arg=()
if [[ -n "$COMPONENT" ]]; then
  selector_arg=(--selector "component=${COMPONENT}")
fi

# Accepted findings (e.g. an upstream chart not supporting a policy) are
# tracked as PolicyExceptions colocated with the component, not silenced
# globally - see components/*/policy-exceptions.yaml and ci/policies/README.md.
shopt -s nullglob
exception_args=()
for f in components/*/policy-exceptions.yaml; do
  exception_args+=(--exception "$f")
done
shopt -u nullglob

echo "Testing base requirements..."
scripts/render.sh local /tmp/rendered-local.yaml --deployed "${selector_arg[@]+"${selector_arg[@]}"}"
kyverno apply ci/policies/base --resource /tmp/rendered-local.yaml "${exception_args[@]}"
echo "All policies passed!"
echo "Testing production requirements (base + production)..."
scripts/render.sh production /tmp/rendered-production.yaml --deployed "${selector_arg[@]+"${selector_arg[@]}"}"
kyverno apply ci/policies/base ci/policies/production --resource /tmp/rendered-production.yaml "${exception_args[@]}"
echo "All production policies passed!"
# The add-ons are in no installation's component list, so neither render above carries one.
# They are held to the same base policies here: an add-on is a workload on the same cluster.
echo "Testing the add-ons..."
scripts/render.sh addons /tmp/rendered-addons.yaml --deployed "${selector_arg[@]+"${selector_arg[@]}"}"
kyverno apply ci/policies/base --resource /tmp/rendered-addons.yaml "${exception_args[@]}"
echo "All add-on policies passed!"
