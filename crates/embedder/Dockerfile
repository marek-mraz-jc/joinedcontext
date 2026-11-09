# syntax=docker/dockerfile:1
# jc-embedder: the embedding model (multilingual-e5-small int8) as a service several deployments
# share (crates/embedder). Build from the repository root:
#   docker build -f crates/embedder/Dockerfile .
# .github/workflows/image-embedder.yml builds, smoke-tests, pushes by digest and signs it.

FROM rust:1.97-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS build
RUN apt-get update && apt-get install -y --no-install-recommends make cmake && rm -rf /var/lib/apt/lists/*
ARG CARGO_BUILD_JOBS=
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
WORKDIR /src
COPY . .
# Only jc-embedder and its dependencies; the target dir goes in the same step, so the layer holds
# only the binary.
RUN cargo build --release --locked -p jc-embedder \
 && strip -o /jc-embedder target/release/jc-embedder \
 && rm -rf target

# The model and ONNX Runtime, each checked against its pinned SHA-256 by the scripts that fetch
# them; jc-embedder checks the model files again before it loads them.
FROM rust:1.97-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS assets
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY vendor/joinedcontext-platform/scripts/ci/e5-small.sh vendor/joinedcontext-platform/scripts/ci/onnxruntime.sh /scripts/
RUN sh /scripts/e5-small.sh /models/e5-small && sh /scripts/onnxruntime.sh /opt/onnxruntime

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab
COPY --from=build /jc-embedder /usr/local/bin/jc-embedder
COPY --from=assets /models/e5-small /models/e5-small
COPY --from=assets /opt/onnxruntime /opt/onnxruntime
USER nonroot:nonroot
EXPOSE 8089 9089
ENV ORT_DYLIB_PATH=/opt/onnxruntime/libonnxruntime.so \
    EMBEDDER_CONFIG=/etc/jc-embedder/embedder.toml
ENTRYPOINT ["/usr/local/bin/jc-embedder"]
