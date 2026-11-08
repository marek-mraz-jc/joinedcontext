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
# An empty dist embeds as nothing and the Portal serves its placeholder page silently: fail here.
RUN cd ui && pnpm build && test -s dist/index.html && ls dist/assets/*.js >/dev/null
RUN cd sdk && pnpm build && test -s dist/kit.js && test -s dist/kit.css && test -s dist/kit-worker.js \
    && test -s dist/runtime/runtime.json && test -s dist/functions-server.js \
    && test -s dist/demos/index.html
ARG JC_COMMIT=
RUN printf '{"commit":"%s","openapi":"%s"}\n' "$JC_COMMIT" "$(sha256sum ui/openapi.json | cut -d' ' -f1)" \
    > ui/dist/version.json

# The build stage's base: the Rust toolchain on the runtime's Debian (same glibc). make for
# tikv-jemalloc-sys, cmake for aws-lc-sys' fallback build.
FROM rust:1.97-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c AS rust
RUN apt-get update && apt-get install -y --no-install-recommends make cmake jq && rm -rf /var/lib/apt/lists/*
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

# What the distroless base lacks, from Debian trixie with each package's dpkg record, so trivy
# reads every version laid over it:
# - git and the libraries it and git-remote-http link that the base does not ship: jc's gateway
#   checks out the organization and project repositories with the git CLI
#   (vendor/joinedcontext-platform jc-manifest assemble.rs), and without it jc stops at start.
# - zlib1g from trixie-security (DSA-6613-1: CVE-2026-76844, CVE-2026-85091, HIGH), as the
#   Portal image lays it (crates/portal/Dockerfile, T-6684), until the pinned base carries it.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2 AS base
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
# The commit, read at run time by the Portal's /api/v1/version (crates/portal/src/api/health.rs):
# set here, in the last layer, rather than compiled in, so a new commit recompiles nothing.
ARG JC_COMMIT=
ENV JC_COMMIT=${JC_COMMIT}
LABEL org.opencontainers.image.revision=${JC_COMMIT} \
      org.opencontainers.image.source=https://github.com/marek-mraz-jc/joinedcontext
ENTRYPOINT ["/usr/local/bin/jc"]
CMD ["--role", "all"]
