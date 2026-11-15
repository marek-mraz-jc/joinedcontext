# Grafana with the NGSI-LD datasource inside the image (T-6504): the project dashboards read their
# Endpoints through it. The plugin is built here from one commit of its public repository, its
# archive checked by hash and its dependencies by the lockfile's, and nothing is fetched when the
# pod starts. Unsigned: grafana.ini allow-lists this one plugin id.
FROM docker.io/library/node:22-alpine@sha256:fe4f6d74e20370acb04ad889077d14d0f64a9c8ea6a42bc60cddf1e844699914 AS plugin
# The commit a person read before it was pinned (T-6504). A move repeats that read: the plugin runs
# in every viewer's browser and its proxy calls carry the person's token.
ADD --checksum=sha256:4f43366b3af97e2899d803d17f51e588b3c6abc35e9cbca78f22c6060b7fcda0 \
    https://github.com/joinedcontext/Grafana-NGSI-LD-data-source/archive/2262d44dc7df597cc11e1cd26421d0abbc1a7284.tar.gz /tmp/plugin.tar.gz
WORKDIR /build
RUN tar -xzf /tmp/plugin.tar.gz --strip-components=3 Grafana-NGSI-LD-data-source-2262d44dc7df597cc11e1cd26421d0abbc1a7284/datasource/plugin \
    && npm ci --ignore-scripts --no-audit --no-fund \
    && npm run build

# A new upstream digest means a new build and a new pin in components/grafana/images.yaml (same tag).
FROM docker.io/grafana/grafana:12.4.14@sha256:ae509d9ae8c1021ca32ec370a9ca975d40165eda9fccd1e1102d317ff8787cd7
# Outside /var/lib/grafana: the chart mounts an emptyDir over that path (no persistence).
COPY --from=plugin /build/dist /usr/share/grafana/jc-plugins/ngsild-grafana-datasource
# The base's OS fixes since upstream built it, as every image here (tests/test_image_os_upgrade.py).
USER root
RUN apk upgrade --no-cache
USER 472
