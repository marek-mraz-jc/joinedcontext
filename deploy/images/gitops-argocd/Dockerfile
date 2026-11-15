# Argo CD as pinned, with the two tools it bundles from other projects replaced (T-4706): v3.5.4
# and v3.5.5 both ship git-lfs 3.7.1 (Go 1.25.3) and kustomize 5.8.1 (Go 1.24.0), CVE-2025-68121
# CRITICAL. Argo CD's own binary is unchanged. Drop this image when an upstream release carries
# fixed builds of both (trivy image --severity CRITICAL on the new tag). Its Ubuntu packages are
# upgraded too: v3.5.4 ships libbrotli1 1.2.0-3build1, CVE-2026-111497 HIGH in CISA KEV (T-6114).
# ponytail: amd64 only, the one architecture the clusters run.
FROM docker.io/library/alpine:3.24.2@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS tools
ARG GIT_LFS=3.8.0 GIT_LFS_SHA256=e455e00f15d9b95661b8d53498ffb0c3367962cf1ec73c31ab7369516cd6ab8d
ARG KUSTOMIZE=5.8.3 KUSTOMIZE_SHA256=cb9e31198d3f63b44848bd0afc4c8efc56e6cfef9c93a4a39a211940e6decd61
# The image has no curl, so the releases are fetched and checked here.
RUN apk add --no-cache curl \
    && mkdir /out && cd /tmp \
    && curl -fsSL --retry 3 --retry-all-errors -o git-lfs.tgz "https://github.com/git-lfs/git-lfs/releases/download/v${GIT_LFS}/git-lfs-linux-amd64-v${GIT_LFS}.tar.gz" \
    && curl -fsSL --retry 3 --retry-all-errors -o kustomize.tgz "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv${KUSTOMIZE}/kustomize_v${KUSTOMIZE}_linux_amd64.tar.gz" \
    && printf '%s  %s\n' "$GIT_LFS_SHA256" git-lfs.tgz "$KUSTOMIZE_SHA256" kustomize.tgz | sha256sum -c - \
    && tar -xzf git-lfs.tgz --strip-components=1 -C /out "git-lfs-${GIT_LFS}/git-lfs" \
    && tar -xzf kustomize.tgz -C /out kustomize \
    && chmod 0755 /out/git-lfs /out/kustomize

FROM quay.io/argoproj/argocd:v3.5.4@sha256:49dff79439bb38b1b942b19a113fe1fec7e6b6c671ddf9fe6a4a7c46bde0b77b
USER root
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y --no-install-recommends \
    && rm -rf /var/lib/apt/lists/*
USER 999
COPY --from=tools /out/git-lfs /out/kustomize /usr/local/bin/
RUN git-lfs version && kustomize version
