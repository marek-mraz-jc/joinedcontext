#!/usr/bin/env bash
set -euo pipefail

# Re-vendors maintained upstream policies from github.com/kyverno/policies
# into base/. To update: bump UPSTREAM_REF, run this script, review the diff
# and run `just test-policies` (expectations live in tests/kyverno-test.yaml).
UPSTREAM_REF=76be98a25d49ae01278a94ecde8f50f9e08577ef

# Note: the PSS baseline policies disallow-capabilities and restrict-seccomp
# are deliberately absent - their restricted counterparts
# (disallow-capabilities-strict, restrict-seccomp-strict) are strict supersets
# and running both would report every violation twice. The same applies to
# best-practices/require-drop-all, which disallow-capabilities-strict subsumes.
UPSTREAM_POLICIES=(
  best-practices/require-ro-rootfs/require-ro-rootfs.yaml
  other/require-image-checksum/require-image-checksum.yaml
  pod-security/baseline/disallow-host-namespaces/disallow-host-namespaces.yaml
  pod-security/baseline/disallow-host-path/disallow-host-path.yaml
  pod-security/baseline/disallow-host-ports/disallow-host-ports.yaml
  pod-security/baseline/disallow-host-process/disallow-host-process.yaml
  pod-security/baseline/disallow-privileged-containers/disallow-privileged-containers.yaml
  pod-security/baseline/disallow-proc-mount/disallow-proc-mount.yaml
  pod-security/baseline/disallow-selinux/disallow-selinux.yaml
  pod-security/baseline/restrict-apparmor-profiles/restrict-apparmor-profiles.yaml
  pod-security/baseline/restrict-sysctls/restrict-sysctls.yaml
  pod-security/restricted/disallow-capabilities-strict/disallow-capabilities-strict.yaml
  pod-security/restricted/disallow-privilege-escalation/disallow-privilege-escalation.yaml
  pod-security/restricted/require-run-as-non-root-user/require-run-as-non-root-user.yaml
  pod-security/restricted/require-run-as-nonroot/require-run-as-nonroot.yaml
  pod-security/restricted/restrict-seccomp-strict/restrict-seccomp-strict.yaml
  pod-security/restricted/restrict-volume-types/restrict-volume-types.yaml
)

# The Pod Security Standards subset that is also DEPLOYED to the cluster by
# components/runtime-policies, so admission and CI judge the same file (T-0008).
# require-drop-all rather than disallow-capabilities-strict on purpose: the strict policy
# also forbids adding capabilities, and Linkerd's linkerd-init adds NET_ADMIN and NET_RAW to
# program the pod's iptables, so at admission it would reject every meshed pod. Rendered
# manifests carry no such init container, which is why CI can hold the stricter line.
RUNTIME_POLICIES=(
  best-practices/require-drop-all/require-drop-all.yaml
  best-practices/require-ro-rootfs/require-ro-rootfs.yaml
  pod-security/restricted/require-run-as-non-root-user/require-run-as-non-root-user.yaml
  pod-security/restricted/require-run-as-nonroot/require-run-as-nonroot.yaml
  # Staged at admission in Audit until dev's PolicyReports are clean (T-2480): the chart's
  # podSecurity.staged list, promoted to podSecurity.policies one by one.
  pod-security/baseline/disallow-privileged-containers/disallow-privileged-containers.yaml
  pod-security/baseline/disallow-host-path/disallow-host-path.yaml
  pod-security/baseline/disallow-host-namespaces/disallow-host-namespaces.yaml
  other/require-image-checksum/require-image-checksum.yaml
  best-practices/require-pod-requests-limits/require-pod-requests-limits.yaml
)
RUNTIME_DEST=../../components/runtime-policies/charts/runtime-policies/files/upstream

cd "$(dirname "$0")"

vendor() {
  local policy="$1" target="$2"
  echo "Vendoring ${policy} -> ${target}"
  {
    echo "---"
    echo "# upstream content is vendored verbatim, so long lines are tolerated:"
    echo "# yamllint disable rule:line-length"
    echo "# Vendored from github.com/kyverno/policies"
    echo "# path: ${policy}"
    echo "# ref: ${UPSTREAM_REF}"
    echo "# Do not edit by hand; re-run ci/policies/vendor-upstream-policies.sh instead."
    curl -fsSL "https://raw.githubusercontent.com/kyverno/policies/${UPSTREAM_REF}/${policy}"
  } > "${target}"
}

targets=()
for policy in "${UPSTREAM_POLICIES[@]}"; do
  target="base/$(basename "$policy")"
  targets+=("${target}")
  vendor "${policy}" "${target}"
done

mkdir -p "${RUNTIME_DEST}"
for policy in "${RUNTIME_POLICIES[@]}"; do
  target="${RUNTIME_DEST}/$(basename "$policy")"
  targets+=("${target}")
  vendor "${policy}" "${target}"
done

# Normalize to the repo's YAML style (pre-commit runs prettier + yamllint).
if command -v npx >/dev/null; then
  npx --yes prettier@3.3.0 --config ../configs/.prettierrc --write "${targets[@]}"
else
  echo "WARNING: npx not found; run pre-commit to normalize formatting." >&2
fi

echo "Done. Review the diff and run: just test-policies"
