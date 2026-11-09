# syntax=docker/dockerfile:1
# The jc image: one binary hosting every module (bins/jc), default features, the Portal UI
# embedded. Build from the repository root: `docker build -f bins/jc/Dockerfile .`
# .github/workflows/image.yml builds, smoke-tests, pushes by digest and signs it. It compiles jc
# on the runner and hands the result in as this file's `build` stage
# (`--build-context build=<dir holding jc, kaisatsu and an empty 0700 state/>`), so only the final
# stage runs there; a plain `docker build` compiles everything below.

# The Portal UI and the App SDK bundles the Portal embeds (rust-embed reads crates/portal/ui/dist
# and crates/portal/sdk/dist at compile time), as crates/portal/Dockerfile builds them.
FROM node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66 AS ui
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /work
# `ui` links `@joinedcontext/sdk` as `link:../sdk`: both lockfiles first, so the install layer
# stays cached while only sources change.
COPY crates/portal/sdk/package.json crates/portal/sdk/pnpm-lock.yaml ./sdk/
COPY crates/portal/ui/package.json crates/portal/ui/pnpm-lock.yaml ./ui/
RUN corepack enable && cd sdk && pnpm install --frozen-lockfile && cd ../ui && pnpm install --frozen-lockfile
COPY crates/portal/sdk/ ./sdk/
COPY crates/portal/ui/ ./ui/
# `tsc -b` type-checks ui/tests, which read the Portal's builder fixture as ../../tests/fixtures/.
COPY crates/portal/tests/fixtures/grafana-builder/ ./tests/fixtures/grafana-builder/
# An empty dist embeds as nothing and the Portal serves its placeholder page silently: fail here.
RUN cd ui && pnpm build && test -s dist/index.html && ls dist/assets/*.js >/dev/null
RUN cd sdk && pnpm build && test -s dist/kit.js && test -s dist/kit.css && test -s dist/kit-worker.js \
    && test -s dist/runtime/runtime.json && test -s dist/functions-server.js \
    && test -s dist/demos/index.html
ARG JC_COMMIT=
RUN printf '{"commit":"%s","openapi":"%s"}\n' "$JC_COMMIT" "$(sha256sum ui/openapi.json | cut -d' ' -f1)" \
    > ui/dist/version.json

# The App bundles the Portal serves under /apps/{name}/: the `apps` stage of
# crates/portal/Dockerfile with crates/portal/ paths (the build contexts differ, so the two cannot
# share a stage; crates/portal/scripts/ci/check-dockerfile-members.py --apps keeps the lists equal).
# The static apps the Portal serves under /apps/{name}/ (AP-14): each built bundle with the
# `integrity.json` the host checks every file against before it serves it (AP-12). An app without
# that map serves nothing, so the stage fails rather than ship one without it (T-2457).
FROM node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66 AS apps
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /work
COPY crates/portal/sdk/package.json crates/portal/sdk/pnpm-lock.yaml ./sdk/
COPY crates/portal/apps/bbsk-ukazovatele/ui/package.json crates/portal/apps/bbsk-ukazovatele/ui/pnpm-lock.yaml ./apps/bbsk-ukazovatele/ui/
COPY crates/portal/apps/banskabystrica-zaznamy/ui/package.json crates/portal/apps/banskabystrica-zaznamy/ui/pnpm-lock.yaml ./apps/banskabystrica-zaznamy/ui/
COPY crates/portal/apps/bbsk-zaznamy/ui/package.json crates/portal/apps/bbsk-zaznamy/ui/pnpm-lock.yaml ./apps/bbsk-zaznamy/ui/
COPY crates/portal/apps/banskabystrica-ovzdusie/ui/package.json crates/portal/apps/banskabystrica-ovzdusie/ui/pnpm-lock.yaml ./apps/banskabystrica-ovzdusie/ui/
COPY crates/portal/apps/banskabystrica-mapa/ui/package.json crates/portal/apps/banskabystrica-mapa/ui/pnpm-lock.yaml ./apps/banskabystrica-mapa/ui/
COPY crates/portal/apps/banskabystrica-data/ui/package.json crates/portal/apps/banskabystrica-data/ui/pnpm-lock.yaml ./apps/banskabystrica-data/ui/
COPY crates/portal/apps/banskabystrica-skoly/ui/package.json crates/portal/apps/banskabystrica-skoly/ui/pnpm-lock.yaml ./apps/banskabystrica-skoly/ui/
COPY crates/portal/apps/bbsk-mapa/ui/package.json crates/portal/apps/bbsk-mapa/ui/pnpm-lock.yaml ./apps/bbsk-mapa/ui/
COPY crates/portal/apps/bbsk-podujatia/ui/package.json crates/portal/apps/bbsk-podujatia/ui/pnpm-lock.yaml ./apps/bbsk-podujatia/ui/
COPY crates/portal/apps/bbsk-data/ui/package.json crates/portal/apps/bbsk-data/ui/pnpm-lock.yaml ./apps/bbsk-data/ui/
COPY crates/portal/apps/bbsk-mosty/ui/package.json crates/portal/apps/bbsk-mosty/ui/pnpm-lock.yaml ./apps/bbsk-mosty/ui/
COPY crates/portal/apps/praha-mapa/ui/package.json crates/portal/apps/praha-mapa/ui/pnpm-lock.yaml ./apps/praha-mapa/ui/
COPY crates/portal/apps/praha-data/ui/package.json crates/portal/apps/praha-data/ui/pnpm-lock.yaml ./apps/praha-data/ui/
COPY crates/portal/apps/praha-odpad/ui/package.json crates/portal/apps/praha-odpad/ui/pnpm-lock.yaml ./apps/praha-odpad/ui/
COPY crates/portal/apps/helsinki-kartta/ui/package.json crates/portal/apps/helsinki-kartta/ui/pnpm-lock.yaml ./apps/helsinki-kartta/ui/
COPY crates/portal/apps/helsinki-data/ui/package.json crates/portal/apps/helsinki-data/ui/pnpm-lock.yaml ./apps/helsinki-data/ui/
COPY crates/portal/apps/bike-stations/ui/package.json crates/portal/apps/bike-stations/ui/pnpm-lock.yaml ./apps/bike-stations/ui/
COPY crates/portal/apps/praha-mesto/ui/package.json crates/portal/apps/praha-mesto/ui/pnpm-lock.yaml ./apps/praha-mesto/ui/
COPY crates/portal/apps/zilina-mapa/ui/package.json crates/portal/apps/zilina-mapa/ui/pnpm-lock.yaml ./apps/zilina-mapa/ui/
COPY crates/portal/apps/zilina-ukazovatele/ui/package.json crates/portal/apps/zilina-ukazovatele/ui/pnpm-lock.yaml ./apps/zilina-ukazovatele/ui/
COPY crates/portal/apps/zilina-zaznamy/ui/package.json crates/portal/apps/zilina-zaznamy/ui/pnpm-lock.yaml ./apps/zilina-zaznamy/ui/
COPY crates/portal/apps/zilina-vyskum/ui/package.json crates/portal/apps/zilina-vyskum/ui/pnpm-lock.yaml ./apps/zilina-vyskum/ui/
RUN corepack enable && cd sdk && pnpm install --frozen-lockfile \
    && cd ../apps/bbsk-ukazovatele/ui && pnpm install --frozen-lockfile \
    && cd ../../banskabystrica-zaznamy/ui && pnpm install --frozen-lockfile \
    && cd ../../bbsk-zaznamy/ui && pnpm install --frozen-lockfile \
    && cd ../../banskabystrica-ovzdusie/ui && pnpm install --frozen-lockfile \
    && cd ../../banskabystrica-mapa/ui && pnpm install --frozen-lockfile \
    && cd ../../banskabystrica-data/ui && pnpm install --frozen-lockfile \
    && cd ../../banskabystrica-skoly/ui && pnpm install --frozen-lockfile \
    && cd ../../bbsk-mapa/ui && pnpm install --frozen-lockfile \
    && cd ../../bbsk-podujatia/ui && pnpm install --frozen-lockfile \
    && cd ../../bbsk-data/ui && pnpm install --frozen-lockfile \
    && cd ../../bbsk-mosty/ui && pnpm install --frozen-lockfile \
    && cd ../../praha-mapa/ui && pnpm install --frozen-lockfile \
    && cd ../../praha-data/ui && pnpm install --frozen-lockfile \
    && cd ../../praha-odpad/ui && pnpm install --frozen-lockfile \
    && cd ../../helsinki-kartta/ui && pnpm install --frozen-lockfile \
    && cd ../../helsinki-data/ui && pnpm install --frozen-lockfile \
    && cd ../../bike-stations/ui && pnpm install --frozen-lockfile \
    && cd ../../praha-mesto/ui && pnpm install --frozen-lockfile \
    && cd ../../zilina-mapa/ui && pnpm install --frozen-lockfile \
    && cd ../../zilina-ukazovatele/ui && pnpm install --frozen-lockfile \
    && cd ../../zilina-zaznamy/ui && pnpm install --frozen-lockfile \
    && cd ../../zilina-vyskum/ui && pnpm install --frozen-lockfile
COPY crates/portal/sdk/ ./sdk/
COPY crates/portal/apps/bbsk-ukazovatele/ui/ ./apps/bbsk-ukazovatele/ui/
COPY crates/portal/apps/banskabystrica-zaznamy/ui/ ./apps/banskabystrica-zaznamy/ui/
COPY crates/portal/apps/bbsk-zaznamy/ui/ ./apps/bbsk-zaznamy/ui/
COPY crates/portal/apps/banskabystrica-ovzdusie/ui/ ./apps/banskabystrica-ovzdusie/ui/
COPY crates/portal/apps/banskabystrica-mapa/ui/ ./apps/banskabystrica-mapa/ui/
COPY crates/portal/apps/banskabystrica-data/ui/ ./apps/banskabystrica-data/ui/
COPY crates/portal/apps/banskabystrica-skoly/ui/ ./apps/banskabystrica-skoly/ui/
COPY crates/portal/apps/bbsk-mapa/ui/ ./apps/bbsk-mapa/ui/
COPY crates/portal/apps/bbsk-podujatia/ui/ ./apps/bbsk-podujatia/ui/
COPY crates/portal/apps/bbsk-data/ui/ ./apps/bbsk-data/ui/
COPY crates/portal/apps/bbsk-mosty/ui/ ./apps/bbsk-mosty/ui/
COPY crates/portal/apps/praha-mapa/ui/ ./apps/praha-mapa/ui/
COPY crates/portal/apps/praha-data/ui/ ./apps/praha-data/ui/
COPY crates/portal/apps/praha-odpad/ui/ ./apps/praha-odpad/ui/
COPY crates/portal/apps/helsinki-kartta/ui/ ./apps/helsinki-kartta/ui/
COPY crates/portal/apps/helsinki-data/ui/ ./apps/helsinki-data/ui/
COPY crates/portal/apps/bike-stations/ui/ ./apps/bike-stations/ui/
COPY crates/portal/apps/praha-mesto/ui/ ./apps/praha-mesto/ui/
COPY crates/portal/apps/zilina-mapa/ui/ ./apps/zilina-mapa/ui/
COPY crates/portal/apps/zilina-ukazovatele/ui/ ./apps/zilina-ukazovatele/ui/
COPY crates/portal/apps/zilina-zaznamy/ui/ ./apps/zilina-zaznamy/ui/
COPY crates/portal/apps/zilina-vyskum/ui/ ./apps/zilina-vyskum/ui/
COPY crates/portal/apps/sharing-probe/bundle/ ./apps/sharing-probe/bundle/
COPY crates/portal/scripts/app-integrity.mjs ./scripts/
RUN cd apps/bbsk-ukazovatele/ui && pnpm build \
    && mkdir -p /srv/apps && cp -r dist /srv/apps/bbsk-ukazovatele \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-ukazovatele
RUN cd apps/banskabystrica-zaznamy/ui && pnpm build \
    && cp -r dist /srv/apps/banskabystrica-zaznamy \
    && node /work/scripts/app-integrity.mjs /srv/apps/banskabystrica-zaznamy
RUN cd apps/bbsk-zaznamy/ui && pnpm build \
    && cp -r dist /srv/apps/bbsk-zaznamy \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-zaznamy
RUN cd apps/banskabystrica-ovzdusie/ui && pnpm build \
    && cp -r dist /srv/apps/banskabystrica-ovzdusie \
    && node /work/scripts/app-integrity.mjs /srv/apps/banskabystrica-ovzdusie
RUN cd apps/banskabystrica-mapa/ui && pnpm build \
    && cp -r dist /srv/apps/banskabystrica-mapa \
    && node /work/scripts/app-integrity.mjs /srv/apps/banskabystrica-mapa
RUN cd apps/banskabystrica-data/ui && pnpm build \
    && cp -r dist /srv/apps/banskabystrica-data \
    && node /work/scripts/app-integrity.mjs /srv/apps/banskabystrica-data
RUN cd apps/banskabystrica-skoly/ui && pnpm build \
    && cp -r dist /srv/apps/banskabystrica-skoly \
    && node /work/scripts/app-integrity.mjs /srv/apps/banskabystrica-skoly
RUN cd apps/bbsk-mapa/ui && pnpm build \
    && cp -r dist /srv/apps/bbsk-mapa \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-mapa
RUN cd apps/bbsk-podujatia/ui && pnpm build \
    && cp -r dist /srv/apps/bbsk-podujatia \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-podujatia
RUN cd apps/bbsk-data/ui && pnpm build \
    && cp -r dist /srv/apps/bbsk-data \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-data
RUN cd apps/bbsk-mosty/ui && pnpm build \
    && cp -r dist /srv/apps/bbsk-mosty \
    && node /work/scripts/app-integrity.mjs /srv/apps/bbsk-mosty
RUN cd apps/praha-mapa/ui && pnpm build \
    && cp -r dist /srv/apps/praha-mapa \
    && node /work/scripts/app-integrity.mjs /srv/apps/praha-mapa
RUN cd apps/praha-data/ui && pnpm build \
    && cp -r dist /srv/apps/praha-data \
    && node /work/scripts/app-integrity.mjs /srv/apps/praha-data
RUN cd apps/praha-odpad/ui && pnpm build \
    && cp -r dist /srv/apps/praha-odpad \
    && node /work/scripts/app-integrity.mjs /srv/apps/praha-odpad
RUN cd apps/helsinki-kartta/ui && pnpm build \
    && cp -r dist /srv/apps/helsinki-kartta \
    && node /work/scripts/app-integrity.mjs /srv/apps/helsinki-kartta
RUN cd apps/helsinki-data/ui && pnpm build \
    && cp -r dist /srv/apps/helsinki-data \
    && node /work/scripts/app-integrity.mjs /srv/apps/helsinki-data
RUN cd apps/bike-stations/ui && pnpm build \
    && cp -r dist /srv/apps/bike-stations \
    && node /work/scripts/app-integrity.mjs /srv/apps/bike-stations
RUN cd apps/praha-mesto/ui && pnpm build \
    && cp -r dist /srv/apps/praha-mesto \
    && node /work/scripts/app-integrity.mjs /srv/apps/praha-mesto
RUN cd apps/zilina-mapa/ui && pnpm build \
    && cp -r dist /srv/apps/zilina-mapa \
    && node /work/scripts/app-integrity.mjs /srv/apps/zilina-mapa
RUN cd apps/zilina-ukazovatele/ui && pnpm build \
    && cp -r dist /srv/apps/zilina-ukazovatele \
    && node /work/scripts/app-integrity.mjs /srv/apps/zilina-ukazovatele
RUN cd apps/zilina-zaznamy/ui && pnpm build \
    && cp -r dist /srv/apps/zilina-zaznamy \
    && node /work/scripts/app-integrity.mjs /srv/apps/zilina-zaznamy
RUN cd apps/zilina-vyskum/ui && pnpm build \
    && cp -r dist /srv/apps/zilina-vyskum \
    && node /work/scripts/app-integrity.mjs /srv/apps/zilina-vyskum
# The App sharing matrix's test page (T-4911): one plain-HTML bundle, no build, shipped under the
# five names whose manifests differ only in visibility (deployment seed/sharing, dev only).
RUN for name in sharing-private sharing-project sharing-organization sharing-roles sharing-public; do \
      cp -r apps/sharing-probe/bundle "/srv/apps/$name" \
      && node /work/scripts/app-integrity.mjs "/srv/apps/$name" || exit 1; \
    done

# The build stage's base: the Rust toolchain on the runtime's Debian (same glibc). make for
# tikv-jemalloc-sys, cmake for aws-lc-sys' fallback build, g++ for the assistant's tokenizer
# (esaxx, oniguruma), curl for the pinned downloads below.
FROM rust:1.97-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS rust
RUN apt-get update && apt-get install -y --no-install-recommends make cmake jq g++ curl ca-certificates && rm -rf /var/lib/apt/lists/*
# The assistant module (G7): the PDFium its crate links, the embedding model and ONNX Runtime,
# each pinned by checksum in its script, as the platform image fetches them.
COPY vendor/joinedcontext-platform/scripts/ci/pdfium.sh vendor/joinedcontext-platform/scripts/ci/e5-small.sh vendor/joinedcontext-platform/scripts/ci/onnxruntime.sh /tmp/
RUN sh /tmp/pdfium.sh /opt/pdfium && sh /tmp/e5-small.sh /opt/e5-small && sh /tmp/onnxruntime.sh /opt/onnxruntime
ENV KREUZBERG_PDFIUM_PREBUILT=/opt/pdfium
# `default` = one job per core (cargo refuses an empty value); a shared build host passes a number.
ARG CARGO_BUILD_JOBS=default
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

# The dependency layer's input: every manifest and lockfile, and a stub file for each target of
# every path package (`cargo metadata` names them, explicit `[[bin]]`/`path =` targets included).
# Each stub has a crate doc line: the Antares crates deny missing_docs in their [lints]. A crate
# [patch.crates-io] replaces keeps its real sources: registry crates compile against it here
# (biblatex uses the patched paste).
# This stage reruns on any change, but its output only changes with a manifest, so the
# `COPY --from=plan` below and the dependency build after it stay cached across source changes.
FROM rust AS plan
WORKDIR /src
COPY . .
RUN set -eu; mkdir /stub; \
    find . \( -name Cargo.toml -o -name Cargo.lock -o -name rust-toolchain.toml -o -path '*/.cargo/config.toml' \) \
      -exec cp --parents {} /stub \; ; \
    cargo metadata --locked --format-version 1 --manifest-path Cargo.toml \
      | jq -r '.packages[] | select(.source == null) | .targets[] | "\(.kind[0]) \(.src_path)"' \
      | while read -r kind path; do \
          rel="${path#/src/}"; mkdir -p "/stub/$(dirname "$rel")"; \
          case "$kind" in lib|rlib|cdylib|staticlib|dylib|proc-macro) echo '//! stub' > "/stub/$rel" ;; \
            *) printf '//! stub\nfn main() {}\n' > "/stub/$rel" ;; esac; \
        done; \
    awk '/^\[patch\.crates-io\]/{p=1;next} /^\[/{p=0} p && /path *=/' Cargo.toml \
      | sed -E 's/.*path *= *"([^"]+)".*/\1/' | while read -r dir; do cp -r --parents "$dir" /stub; done

FROM rust AS build
WORKDIR /src
COPY --from=plan /stub ./
# Every registry and git dependency of jc (and of the kaisatsu CLI below), compiled once over the
# stub sources.
RUN cargo build --release --locked -p jc -p kaisatsu-server
COPY . .
COPY --from=ui /work/ui/dist ./crates/portal/ui/dist
COPY --from=ui /work/sdk/dist ./crates/portal/sdk/dist
# COPY keeps the context's mtimes, which can be older than the stub build: touch every Rust source
# so cargo rebuilds the path packages over the real code. The target dir goes in the same step, so
# the layer holds only the binary.
RUN find bins crates vendor -name '*.rs' -not -path '*/node_modules/*' -exec touch {} + \
 && cargo build --release --locked -p jc -p kaisatsu-server \
 && strip -o /jc target/release/jc \
 && strip -o /kaisatsu target/release/kaisatsu \
 && rm -rf target \
 && mkdir -p /state && chmod 0700 /state

# What the distroless base lacks, from Debian trixie with each package's dpkg record, so trivy
# reads every version laid over it:
# - git and the libraries it and git-remote-http link that the base does not ship: jc's gateway
#   checks out the organization and project repositories with the git CLI
#   (vendor/joinedcontext-platform jc-manifest assemble.rs), and without it jc stops at start.
# - zlib1g from trixie-security (DSA-6613-1: CVE-2026-76844, CVE-2026-85091, HIGH), as the
#   Portal image lays it (crates/portal/Dockerfile, T-6684), until the pinned base carries it.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab AS base
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS overlay
COPY --from=base /var/lib/dpkg/status.d /base-status
RUN set -eu; apt-get update; \
    apt-get install -y --no-install-recommends git; \
    apt-get install -y --no-install-recommends --only-upgrade zlib1g; \
    dpkg --compare-versions "$(dpkg-query -W -f='${Version}' zlib1g)" ge 1:1.3.dfsg+really1.3.1-1+deb13u1; \
    arch=$(dpkg --print-architecture); \
    libs=$(for f in /usr/bin/git /usr/lib/git-core/git-remote-http; do ldd "$f" | awk '/=>/ {print $3}'; done | sort -u); \
    pkgs=$( (echo git; echo zlib1g; for l in $libs; do dpkg -S "$(readlink -f "$l")" | cut -d: -f1; done) | sort -u); \
    mkdir -p /overlay/var/lib/dpkg/status.d; : > /files; \
    for p in $pkgs; do \
      # a package the base ships already stays the base's, zlib1g (the fix) excepted
      if [ "$p" != zlib1g ] && [ -e "/base-status/$p" ]; then continue; fi; \
      dpkg -L "$p" | grep -vE '^/usr/share/(doc|man|locale|lintian|bash-completion)/' \
        | while read -r f; do if [ -f "$f" ] || [ -L "$f" ]; then echo "$f"; fi; done >> /files; \
      dpkg-query -s "$p" | grep -v '^Status:' > "/overlay/var/lib/dpkg/status.d/$p"; \
      md5="/var/lib/dpkg/info/$p:$arch.md5sums"; [ -f "$md5" ] || md5="/var/lib/dpkg/info/$p.md5sums"; \
      if [ -f "$md5" ]; then cp "$md5" "/overlay/var/lib/dpkg/status.d/$p.md5sums"; fi; \
      echo "overlay: $p $(dpkg-query -W -f='${Version}' "$p")"; \
    done; \
    # tar keeps git-core's hard links as links (one copy of the git binary, not ~140)
    tar -cf - -T /files | tar -xf - -C /overlay; \
    rm -rf /var/lib/apt/lists/*

FROM base
COPY --from=overlay /overlay/ /
COPY --from=build /jc /usr/local/bin/jc
# kaisatsu's own CLI, for the one step jc has no equivalent of: `kaisatsu migrate` as the
# kaisatsu_migrate role, before jc connects as kaisatsu_app (an init container in components/jc).
COPY --from=build /kaisatsu /usr/local/bin/kaisatsu
COPY --from=apps /srv/apps /srv/apps
# The assistant's embedding model and ONNX Runtime; without them the module stays off. From
# `rust`, which downloads them: ship hands in `build` as a named context holding only the
# binaries and state/ (image.yml), so a COPY from `build` finds nothing there.
COPY --from=rust /opt/e5-small /opt/e5-small
COPY --from=rust /opt/onnxruntime/libonnxruntime.so /usr/local/lib/libonnxruntime.so
ENV JC_ASSISTANT_MODEL_DIR=/opt/e5-small ORT_DYLIB_PATH=/usr/local/lib/libonnxruntime.so
# The state dir (generated secrets, JCBIN_STATE_DIR) owned by nonroot and 0700; in a pod a volume
# goes over it and the root filesystem stays read-only (/tmp is an emptyDir there too).
COPY --from=build --chown=65532:65532 /state /var/lib/jc
USER nonroot:nonroot
EXPOSE 8080
ENV JCBIN_LISTEN=0.0.0.0:8080 \
    JCBIN_STATE_DIR=/var/lib/jc \
    TMPDIR=/tmp
# The bundles above, read from disk per request (the Portal's static host); a deployment that
# mounts its own artifact root sets it elsewhere.
ENV JC_PORTAL_APPS_DIR=/srv/apps
# jemalloc as the Portal and the gateway run it (crates/portal/Dockerfile): freed pages purged after a
# second, four arenas whatever the node's core count.
ENV _RJEM_MALLOC_CONF=background_thread:true,narenas:4,dirty_decay_ms:1000,muzzy_decay_ms:0
# The commit, read at run time by the Portal's /api/v1/version (crates/portal/src/api/health.rs):
# set here, in the last layer, rather than compiled in, so a new commit recompiles nothing.
ARG JC_COMMIT=
ENV JC_COMMIT=${JC_COMMIT}
LABEL org.opencontainers.image.revision=${JC_COMMIT} \
      org.opencontainers.image.source=https://github.com/marek-mraz-jc/joinedcontext
ENTRYPOINT ["/usr/local/bin/jc"]
CMD ["--role", "all"]
