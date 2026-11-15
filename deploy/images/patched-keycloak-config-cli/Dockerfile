# keycloak-config-cli as pinned, with its Ubuntu packages upgraded (T-6114): 6.5.1-26 ships
# libbrotli1 1.2.0-3build1, CVE-2026-111497 HIGH and in CISA KEV, and no newer tag rebuilds the
# base. Drop this image when an upstream tag scans clean (trivy image --pkg-types os).
FROM docker.io/adorsys/keycloak-config-cli:6.5.1-26@sha256:1b22dfaa9ae0c71f74b0342f9221a6510f272da5def683dbba26a98e6b1b1411
USER root
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y --no-install-recommends \
    && rm -rf /var/lib/apt/lists/*
USER 65534
