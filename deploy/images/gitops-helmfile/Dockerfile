# Argo CD's helmfile plugin sidecar and the smoke gate's image (ADR-N-056 §2.2, §2.6; T-4706):
# what gitops/cmp/generate.sh and gitops/smoke/files/gate.sh run, and nothing else. The stock
# helmfile image bundles helm-s3, sops, age, kubectl and kustomize built with old Go (CVE-2025-68121
# and CVE-2026-33186, CRITICAL); none of them is used here but kubectl, which comes as its own
# release. Each binary is a release download checked against the sha256 pinned below.
FROM docker.io/library/alpine:3.24.3@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a

# The versions CI renders with (helmfile 1.8.1, helm 4.3.0; .github/workflows/ci.yml).
ARG HELMFILE=1.8.1 HELMFILE_SHA256=ccabbe75dddbc25008d055fe92a44f95a184342211e7244d32b870409239c7a3
ARG HELM=4.3.0 HELM_SHA256=86584a54def73570558f66f5111cc53dfed56689637ae32c1201205d494f54fb
ARG KUBECTL=1.35.9 KUBECTL_SHA256=3cfeaf80be482b435b0aa214aff6e0b2c312ee23c0ff20810c75517b6004c6eb
ARG YQ=4.54.1 YQ_SHA256=8e34fc298390875de416e6a4afcb8cabeceb25d9aa8506c1a2f9353cf702ea5f

# ponytail: amd64 only, the one architecture the clusters run; add a TARGETARCH table if one
# ever runs arm64.
RUN apk upgrade --no-cache \
    && apk add --no-cache bash ca-certificates curl jq \
    && cd /tmp \
    && curl -fsSL --retry 3 --retry-all-errors -o helmfile.tgz "https://github.com/helmfile/helmfile/releases/download/v${HELMFILE}/helmfile_${HELMFILE}_linux_amd64.tar.gz" \
    && curl -fsSL --retry 3 --retry-all-errors -o helm.tgz "https://get.helm.sh/helm-v${HELM}-linux-amd64.tar.gz" \
    && curl -fsSL --retry 3 --retry-all-errors -o kubectl "https://dl.k8s.io/release/v${KUBECTL}/bin/linux/amd64/kubectl" \
    && curl -fsSL --retry 3 --retry-all-errors -o yq "https://github.com/mikefarah/yq/releases/download/v${YQ}/yq_linux_amd64" \
    && printf '%s  %s\n' "$HELMFILE_SHA256" helmfile.tgz "$HELM_SHA256" helm.tgz \
        "$KUBECTL_SHA256" kubectl "$YQ_SHA256" yq | sha256sum -c - \
    && tar -xzf helmfile.tgz -C /usr/local/bin helmfile \
    && tar -xzf helm.tgz --strip-components=1 -C /usr/local/bin linux-amd64/helm \
    && install -m 0755 kubectl yq /usr/local/bin/ \
    && rm -f /tmp/* \
    && helmfile --version && helm version --short && kubectl version --client && yq --version

# Argo CD runs the sidecar as its own 999 (components/gitops/values/argocd).
USER 999
