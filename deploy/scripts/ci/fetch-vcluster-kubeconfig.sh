#!/usr/bin/env bash
# =============================================================================
# fetch-vcluster-kubeconfig.sh
#
# Retrieves the kubeconfig of a loft.sh vcluster running on the host cluster
# and installs it as the active kubeconfig for subsequent kubectl/helm calls.
#
# Required environment:
#   PIPELINE_SLUG         release/namespace suffix used by deploy-branch/deploy-mr

#                         ($CI_COMMIT_REF_SLUG for branch pipelines,
#                          mr-$CI_MERGE_REQUEST_IID for MR pipelines)
#   SMOKE_TEST_DOMAIN     expected server hostname in the kubeconfig
#
# Optional:
#   VCLUSTER_POLL_ATTEMPTS (default 30)
#   VCLUSTER_POLL_INTERVAL (default 10, seconds)
#   VCLUSTER_KUBECONFIG    (default /tmp/vcluster-kubeconfig)
# =============================================================================
set -Eeuo pipefail

: "${PIPELINE_SLUG:?PIPELINE_SLUG must be set}"
: "${SMOKE_TEST_DOMAIN:?SMOKE_TEST_DOMAIN must be set}"

ATTEMPTS="${VCLUSTER_POLL_ATTEMPTS:-30}"
INTERVAL="${VCLUSTER_POLL_INTERVAL:-10}"
VCLUSTER_KUBECONFIG="${VCLUSTER_KUBECONFIG:-/tmp/vcluster-kubeconfig}"

HOST_NS="smoke-test-${PIPELINE_SLUG}"
SECRET_NAME="vc-smoke-test-${PIPELINE_SLUG}"

echo "Waiting for vcluster kubeconfig secret ${HOST_NS}/${SECRET_NAME}..."
for i in $(seq 1 "${ATTEMPTS}"); do
  kubectl -n "${HOST_NS}" get secret "${SECRET_NAME}" \
    -o jsonpath='{.data.config}' 2>/dev/null \
    | base64 -d 2>/dev/null > "${VCLUSTER_KUBECONFIG}.tmp" || true

  if grep -q "${SMOKE_TEST_DOMAIN}" "${VCLUSTER_KUBECONFIG}.tmp" 2>/dev/null \
      && KUBECONFIG="${VCLUSTER_KUBECONFIG}.tmp" kubectl version --request-timeout=5s >/dev/null 2>&1; then
    mv "${VCLUSTER_KUBECONFIG}.tmp" "${VCLUSTER_KUBECONFIG}"
    echo "Kubeconfig ready and vcluster reachable after ${i} attempt(s)"
    break
  fi

  if [ "${i}" -eq "${ATTEMPTS}" ]; then
    echo "ERROR: timed out waiting for vcluster kubeconfig / API server" >&2
    exit 1
  fi
  echo "Attempt ${i}/${ATTEMPTS} - vcluster not ready yet, waiting ${INTERVAL}s..."
  sleep "${INTERVAL}"
done

mkdir -p ~/.kube
mv "${VCLUSTER_KUBECONFIG}" ~/.kube/config
chmod 600 ~/.kube/config
kubectl cluster-info
