# syntax=docker/dockerfile:1
# The jc image: one binary hosting every module (bins/jc), default features, the Portal UI
# embedded. Build from the repository root: `docker build -f bins/jc/Dockerfile .`
# .github/workflows/image.yml builds, smoke-tests, pushes by digest and signs it.

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
# An empty dist embeds as nothing and the Portal serves its placeholder page silently: fail here.
RUN cd ui && pnpm build && test -s dist/index.html && ls dist/assets/*.js >/dev/null
RUN cd sdk && pnpm build && test -s dist/kit.js && test -s dist/kit.css && test -s dist/kit-worker.js \
    && test -s dist/runtime/runtime.json && test -s dist/functions-server.js \
    && test -s dist/demos/index.html
ARG JC_COMMIT=
RUN printf '{"commit":"%s","openapi":"%s"}\n' "$JC_COMMIT" "$(sha256sum ui/openapi.json | cut -d' ' -f1)" \
    > ui/dist/version.json

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
# Empty = cargo's default (one job per core); a shared build host passes a lower number.
ARG CARGO_BUILD_JOBS=
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

# The dependency layer's input: every manifest and lockfile, and an empty file for each target of
# every path package (`cargo metadata` names them, explicit `[[bin]]`/`path =` targets included).
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
          case "$kind" in lib|rlib|cdylib|staticlib|dylib|proc-macro) : > "/stub/$rel" ;; \
            *) echo 'fn main() {}' > "/stub/$rel" ;; esac; \
        done

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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
COPY --from=build /jc /usr/local/bin/jc
# kaisatsu's own CLI, for the one step jc has no equivalent of: `kaisatsu migrate` as the
# kaisatsu_migrate role, before jc connects as kaisatsu_app (an init container in components/jc).
COPY --from=build /kaisatsu /usr/local/bin/kaisatsu
# The assistant's embedding model and ONNX Runtime; without them the module stays off.
COPY --from=build /opt/e5-small /opt/e5-small
COPY --from=build /opt/onnxruntime/libonnxruntime.so /usr/local/lib/libonnxruntime.so
ENV JC_ASSISTANT_MODEL_DIR=/opt/e5-small ORT_DYLIB_PATH=/usr/local/lib/libonnxruntime.so
# The state dir (generated secrets, JCBIN_STATE_DIR) owned by nonroot and 0700; in a pod a volume
# goes over it and the root filesystem stays read-only (/tmp is an emptyDir there too).
COPY --from=build --chown=65532:65532 /state /var/lib/jc
USER nonroot:nonroot
EXPOSE 8080
ENV JCBIN_LISTEN=0.0.0.0:8080 \
    JCBIN_STATE_DIR=/var/lib/jc \
    TMPDIR=/tmp
# jemalloc as the Portal and the gateway run it (crates/portal/Dockerfile): freed pages purged after a
# second, four arenas whatever the node's core count.
ENV _RJEM_MALLOC_CONF=background_thread:true,narenas:4,dirty_decay_ms:1000,muzzy_decay_ms:0
ENTRYPOINT ["/usr/local/bin/jc"]
CMD ["--role", "all"]
