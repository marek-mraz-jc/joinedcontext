# CKAN's Solr as pinned, with its Ubuntu packages upgraded (T-6114): 2.11-solr9 ships libbrotli1
# 1.0.9-2build6, CVE-2026-111497 HIGH and in CISA KEV, and no newer tag rebuilds the base. The
# Solr jars stay as shipped (their excuses are in ci/pinned-images-ignore.yaml). Drop this image
# when an upstream tag scans clean (trivy image --pkg-types os).
FROM docker.io/ckan/ckan-solr:2.11-solr9@sha256:ef8e5d3e6be1678cfe50ceb14a8ebf16c36d44aac2de1086a285ce6807af64e6
USER root
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y --no-install-recommends \
    && rm -rf /var/lib/apt/lists/*
USER solr
