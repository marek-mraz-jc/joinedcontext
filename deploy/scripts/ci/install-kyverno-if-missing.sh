#!/usr/bin/env bash
# =============================================================================
# install-kyverno-if-missing.sh
#
# Installs Kyverno into the current cluster, but only if it is not already
# installed. Re-installing over an existing Kyverno would interrupt the
# admission webhook and cause transient failures for in-flight requests, so
# we short-circuit on "already installed".
#
# Assumes `helm` and `helmfile` are on PATH.
# =============================================================================
set -Eeuo pipefail

if helm --namespace kyverno status kyverno >/dev/null 2>&1; then
  echo "Kyverno already installed — skipping."
  exit 0
fi

echo "Kyverno not installed yet — bootstrapping."
helmfile -f ci/kyverno/helmfile.yaml.gotmpl sync
