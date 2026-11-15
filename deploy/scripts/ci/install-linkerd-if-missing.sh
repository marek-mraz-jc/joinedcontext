#!/usr/bin/env bash
# =============================================================================
# install-linkerd-if-missing.sh
#
# Installs Linkerd (CRDs + control plane) into the current cluster with a
# freshly generated trust anchor / issuer pair, but only if it's not already
# installed.
#
# Re-installing over an existing Linkerd rotates the trust anchor, which
# invalidates all mesh certs for pods that were injected with the previous
# anchor. On a pipeline re-run against an already-deployed vcluster that
# silently breaks every meshed workload, so we short-circuit on "already
# installed".
#
# Assumes `kubectl`, `helm`, `helmfile`, and `step` (smallstep CLI) are on
# PATH. `step` is installed on demand if missing (see fallback block).
# =============================================================================
set -Eeuo pipefail

if helm --namespace linkerd status linkerd-control-plane >/dev/null 2>&1; then
  echo "Linkerd control plane already installed — skipping (re-install would rotate the trust anchor and break existing meshed pods)."
  exit 0
fi

echo "Linkerd not installed yet — bootstrapping control plane with a fresh trust anchor."

if ! command -v step >/dev/null 2>&1; then
  # Prefer the Alpine package (this script currently runs on the Alpine-based
  # cicd image); fall back to the upstream release tarball on other bases.
  # TODO: bake step-cli into container-images/cicd so this runtime install
  # can go away.
  if ! apk add --no-cache step-cli 2>/dev/null; then
    STEP_VERSION=0.28.6
    STEP_URL="https://github.com/smallstep/cli/releases/download"
    STEP_FILE="step_linux_${STEP_VERSION}_amd64.tar.gz"
    curl -fsSL "${STEP_URL}/v${STEP_VERSION}/${STEP_FILE}" -o /tmp/step.tar.gz
    tar -xzf /tmp/step.tar.gz -C /tmp
    install /tmp/step_${STEP_VERSION}/bin/step /usr/local/bin/step
    rm -rf /tmp/step*
  fi
fi

echo "Generating Linkerd trust anchor and issuer certificates..."
# --not-after=8760h (1 year) is fine for ephemeral smoke-test vclusters —
# they're destroyed long before the cert expires.
step certificate create root.linkerd.cluster.local /tmp/ca.crt /tmp/ca.key \
  --profile root-ca --no-password --insecure --not-after=8760h
step certificate create identity.linkerd.cluster.local /tmp/issuer.crt /tmp/issuer.key \
  --profile intermediate-ca --no-password --insecure --not-after=8760h \
  --ca /tmp/ca.crt --ca-key /tmp/ca.key

export LINKERD_TRUST_ANCHOR_PEM
export LINKERD_ISSUER_CRT_PEM
export LINKERD_ISSUER_KEY_PEM
LINKERD_TRUST_ANCHOR_PEM="$(cat /tmp/ca.crt)"
LINKERD_ISSUER_CRT_PEM="$(cat /tmp/issuer.crt)"
LINKERD_ISSUER_KEY_PEM="$(cat /tmp/issuer.key)"

helmfile -f ci/linkerd/helmfile.yaml.gotmpl sync
