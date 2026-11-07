# syntax=docker/dockerfile:1
# The build lane's image (AP-80, AP-82, ADR-N-026, ADR-N-028): git, curl, node, this release's SDK and the
# template's dependencies installed once, here, so a build fetches no package. Built from the
# repository root: docker build -f builder/Dockerfile . (the Node runner), and with
# --target rust the rust-1.90 runner of fullstack Apps (AP-106).

FROM node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66 AS sdk
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /sdk
COPY sdk/package.json sdk/pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY sdk/ ./
RUN pnpm pack --pack-destination /out && mv /out/*.tgz /out/sdk.tgz \
    && node -p 'require("./package.json").version' > /out/sdk-version

FROM node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66 AS template
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
COPY --from=sdk /out/ /opt/sdk/
# The SDK as this release built it, never a registry copy of the same version number, unpacked
# with the template beside it as in the repository: the template's lockfile resolves the SDK
# from the directory above (`file:..`, AP-126).
RUN mkdir -p /build/sdk/template && tar -xzf /opt/sdk/sdk.tgz --strip-components=1 -C /build/sdk
WORKDIR /build/sdk/template
COPY sdk/template/package.json sdk/template/pnpm-lock.yaml ./
COPY builder/lane.mjs /build/lane.mjs
# Exactly the packages the lockfile names (AP-127): a lockfile out of step with package.json
# fails the install, and a store missing a package the lockfile names fails the check.
RUN node /build/lane.mjs lock-manifest . \
    && corepack enable && pnpm install --frozen-lockfile --ignore-scripts \
    && node /build/lane.mjs store-check pnpm-lock.yaml node_modules \
    && test -x node_modules/.bin/vite && test -x node_modules/.bin/vitest && test -x node_modules/.bin/tsc \
    && cp /opt/sdk/sdk-version ./sdk-version \
    && mv /build/sdk/template /opt/template

# The forge's runner binary (ADR-N-028 §3.3), built from its release's tagged commit: the
# published 3.5.0 binary (and 4.x, 5.0.0) is linked with go1.27.1 and golang.org/x/net v0.59.0,
# whose HIGH CVEs (CVE-2026-78667, -78669, -97031) trivy refuses. Back to the release download
# with --checksum once a release ships go >= 1.27.2 and x/net >= 0.60.0 (T-3469). -buildvcs=false
# stamps the module `(devel)` as the release does: a commit-only checkout would stamp a v0.0.0
# pseudo-version that trivy reads as older than CVE-2026-73802's fix (34bfa19), which v3.5.0 holds.
# go >= 1.27.3 for CVE-2026-111943 (crypto/x509; owner cooldown exception 2026-10-21, T-5026).
FROM golang:1.27.3-trixie@sha256:604106102d33b1084e37ca08adf98ac0813624ae8595bd0d7f3965ecebded09c AS runner
ARG RUNNER_VERSION=3.5.0
ARG RUNNER_COMMIT=699a97fc8cbd58e1381717710a7431aac70893a3
WORKDIR /src
RUN git init -q . && git fetch -q --depth 1 https://gitea.com/gitea/runner.git ${RUNNER_COMMIT} \
    && git checkout -q FETCH_HEAD && test "$(git rev-parse HEAD)" = "${RUNNER_COMMIT}" \
    && go get golang.org/x/net@v0.60.0 && go mod tidy \
    && CGO_ENABLED=0 go build -trimpath -buildvcs=false -tags 'netgo osusergo' \
       -ldflags "-s -w -X gitea.com/gitea/runner/internal/pkg/ver.version=v${RUNNER_VERSION}" -o /gitea-runner . \
    && go version -m /gitea-runner | grep -E '^/gitea-runner: go1\.27\.([3-9]|[1-9][0-9])$' \
    && go version -m /gitea-runner | grep -qE 'golang.org/x/net[[:space:]]+v0\.(6[0-9]|[7-9][0-9])\.'

# The crate store of the rust-1.90 runner (AP-106): every crate this release's Cargo.lock names,
# the reference applications' included, fetched once here so a fullstack build fetches none.
FROM rust:1.90-slim-trixie@sha256:7fa728f3678acf5980d5db70960cf8491aff9411976789086676bdf0c19db39e AS crates
WORKDIR /src
COPY . .
# The SDK's React + Rust/WebAssembly example and the Apps with a WebAssembly part keep their own
# lock (AP-142), as do the server components of `wasm` Apps (AP-151) and the wasm template a
# generated App's server starts from (T-3575): their crates too.
RUN mkdir -p /opt/cargo/git && CARGO_HOME=/opt/cargo cargo fetch --locked \
    && for crate in sdk/examples/react-rust-wasm/wasm apps/*/wasm sdk/examples/*/server sdk/template-wasm/server apps/*/server; do \
         [ -f "$crate/Cargo.toml" ] || continue; \
         CARGO_HOME=/opt/cargo cargo fetch --locked --manifest-path "$crate/Cargo.toml" || exit 1; \
       done \
    && find /opt/cargo/git/checkouts -name Cargo.lock -delete
# A git dependency is checked out whole, its own Cargo.lock included, and cargo never reads that
# lock: the crates a build uses are the ones this repository's locks name, fetched above. Left in
# the store it describes another project's build (the Antares checkout's rumqttc pins
# rustls-webpki 0.102.8, GHSA-82j2-j2ch-gfr8), which trivy and the attested SBOM would report as
# installed here (T-3567).

# The toolchain of the rust-1.90 runner. The tag pins the toolchain, not its Debian: upgrade the
# base's packages so the image carries the security fixes trixie has shipped since the tag was
# cut (the image lane's trivy gate).
FROM rust:1.90-slim-trixie@sha256:7fa728f3678acf5980d5db70960cf8491aff9411976789086676bdf0c19db39e AS toolchain
# APT_REFRESH: the UTC day, set by image.yml, so the apt layer below leaves the build cache once a
# day and picks up the Debian security fixes released since (vulnerabilities lane, T-3834).
ARG APT_REFRESH
RUN apt-get update && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends git curl ca-certificates musl-tools \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl wasm32-unknown-unknown wasm32-wasip2

# The bindings of a `ui` App's WebAssembly part (AP-142): one wasm-bindgen-cli, pinned here; an
# App's wasm/Cargo.lock names this version or the lane refuses it (builder/build-wasm.sh).
FROM toolchain AS wasm-bindgen
ARG WASM_BINDGEN_VERSION=0.2.129
RUN cargo install --locked --root /opt/wasm-bindgen wasm-bindgen-cli --version ${WASM_BINDGEN_VERSION}

# The dependencies of the reference fullstack apps, compiled here, by CI, from their own
# Cargo.lock (AP-106, T-2794): the lane's two builds, `cargo test` and the release musl binary,
# with the lane's own paths, so the fingerprints match and a job only compiles its own crate and
# what its lock does not share. Cargo-chef style: each app's manifest and lock over stub sources,
# so nothing of the apps' code is in the seed.
FROM toolchain AS seed
COPY --from=crates /opt/cargo/registry/index /opt/cargo/registry/index
COPY --from=crates /opt/cargo/registry/cache /opt/cargo/registry/cache
COPY apps/air-quality/Cargo.toml apps/air-quality/Cargo.lock /manifests/air-quality/
COPY apps/hsl-transport/Cargo.toml apps/hsl-transport/Cargo.lock /manifests/hsl-transport/
ENV CARGO_HOME=/tmp/build/cargo CARGO_TARGET_DIR=/tmp/build/target CARGO_NET_OFFLINE=true
RUN set -eu; for app in air-quality hsl-transport; do \
      rm -rf /tmp/build/app /tmp/build/cargo && mkdir -p /tmp/build/cargo/registry /tmp/build/app/src; \
      ln -s /opt/cargo/registry/index /opt/cargo/registry/cache /tmp/build/cargo/registry/; \
      cp /manifests/$app/Cargo.toml /manifests/$app/Cargo.lock /tmp/build/app/; \
      echo 'fn main() {}' > /tmp/build/app/src/main.rs; : > /tmp/build/app/src/lib.rs; \
      cd /tmp/build/app; \
      cargo test --offline --locked --no-run; \
      cargo build --release --offline --locked --target x86_64-unknown-linux-musl; \
    done; \
    # The stubs' own artifacts are no App's: only the dependencies stay.
    find /tmp/build/target -depth \( -name 'air-quality*' -o -name 'air_quality*' -o -name 'libair_quality*' \
      -o -name 'hsl-transport*' -o -name 'hsl_transport*' -o -name 'libhsl_transport*' \) -exec rm -rf {} +; \
    rm -rf /tmp/build/target/*/incremental /tmp/build/target/*/*/incremental; \
    mv /tmp/build/target /opt/cargo/target-seed
# The dependencies of a generated `wasm` App's server tests (T-3618): the wasm template's lock,
# `cargo test` on the host only, kept apart so the test sandbox copies these and nothing else.
# The same paths as the sandbox's (builder/lane.mjs `CARGO`), the git store linked as it does.
COPY --from=crates /opt/cargo/git /opt/cargo/git
COPY sdk/template-wasm/server/Cargo.toml sdk/template-wasm/server/Cargo.lock /manifests/app-server/
RUN set -eu; rm -rf /tmp/build/app /tmp/build/cargo /tmp/build/target; \
    mkdir -p /tmp/build/cargo/registry /tmp/build/app/src; \
    ln -s /opt/cargo/registry/index /opt/cargo/registry/cache /tmp/build/cargo/registry/; \
    ln -s /opt/cargo/git /tmp/build/cargo/git; \
    cp /manifests/app-server/Cargo.toml /manifests/app-server/Cargo.lock /tmp/build/app/; \
    : > /tmp/build/app/src/lib.rs; \
    cd /tmp/build/app && cargo test --offline --locked --no-run; \
    find /tmp/build/target -depth \( -name 'app-server*' -o -name 'app_server*' -o -name 'libapp_server*' \) -exec rm -rf {} +; \
    rm -rf /tmp/build/target/debug/incremental; \
    mv /tmp/build/target /opt/cargo/test-seed

# The rust-1.90 runner (AP-105, AP-106): the toolchain with the musl target, node and the
# template for the interface, the crate store, the precompiled dependencies, and the same lane and
# runner as the Node one. Built with `--target rust`; the last stage stays the Node runner.
FROM toolchain AS rust
COPY --from=node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66 /usr/local/bin/node /usr/local/bin/node
COPY --from=template /opt/template/ /opt/template/
COPY --from=crates /opt/cargo/registry/index /opt/cargo/registry/index
COPY --from=crates /opt/cargo/registry/cache /opt/cargo/registry/cache
# The git sources the locks name: the guest SDK `jc-app-sdk` of the `wasm` Apps' server/ (T-3346).
COPY --from=crates /opt/cargo/git /opt/cargo/git
# Read-only to the lane's uid: a job copies it and never writes it.
COPY --from=seed /opt/cargo/target-seed /opt/cargo/target-seed
COPY --from=seed /opt/cargo/test-seed /opt/cargo/test-seed
COPY scripts/app-integrity.mjs builder/lane.mjs /opt/template/
COPY builder/build-app.sh /usr/local/bin/build-app
COPY builder/build-wasm.sh /usr/local/bin/build-wasm
COPY builder/build-component.sh /usr/local/bin/build-component
COPY --from=wasm-bindgen /opt/wasm-bindgen/bin/wasm-bindgen /usr/local/bin/wasm-bindgen
# The browser checks of a ui App built here (AP-142, T-2827), as on the Node runner.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN /opt/template/node_modules/.bin/playwright install --with-deps --only-shell chromium \
    && rm -rf /var/lib/apt/lists/* \
    && test -n "$(find /opt/ms-playwright -name chrome-headless-shell -type f)"
COPY --from=runner --chmod=0755 /gitea-runner /usr/local/bin/gitea-runner
COPY builder/runner.sh /usr/local/bin/runner
COPY builder/runner.yaml /opt/runner/runner.yaml
# The same runner, listed under the label a fullstack workflow asks for.
# A release build and link take longer than an interface build: the job's own 30 minutes.
RUN sed -i -e 's/"node-22:host"/"rust-1.90:host"/' -e 's/timeout: 20m/timeout: 30m/' /opt/runner/runner.yaml \
    && grep -q '"rust-1.90:host"' /opt/runner/runner.yaml && grep -q 'timeout: 30m' /opt/runner/runner.yaml
USER 1000:1000
WORKDIR /tmp
ENTRYPOINT ["/usr/local/bin/build-app"]

FROM node:24-trixie-slim@sha256:173f125896c3b47ddf056734c7ea789d04595a6a08769a8f78e0df642781fb66
# npm, npx and corepack ship in the base image and nothing here runs them: a build installs
# nothing (AP-82). Removed, so their bundled packages are neither attack surface nor trivy findings.
ARG APT_REFRESH
RUN apt-get update && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends git curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rm -rf /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/corepack \
      /usr/local/bin/npm /usr/local/bin/npx /usr/local/bin/corepack
COPY --from=template /opt/template/ /opt/template/
# The browser of the browser checks (T-2827): Chromium's headless shell at the build the
# template's Playwright pins, with the libraries it links, installed at image build. A build only
# starts it while its checks run, inside the App's own build pod, and it loads the bundle alone.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN /opt/template/node_modules/.bin/playwright install --with-deps --only-shell chromium \
    && rm -rf /var/lib/apt/lists/* \
    && test -n "$(find /opt/ms-playwright -name chrome-headless-shell -type f)"
COPY scripts/app-integrity.mjs builder/lane.mjs /opt/template/
COPY builder/build-app.sh /usr/local/bin/build-app
# `jcctl validate --project`, the check every project repository's merge requests run on this
# runner (CC-90, .gitea/workflows/validate.yml of the project template). The platform image's
# own binary, pinned by digest: it links libc and libgcc alone, both in this base.
COPY --from=ghcr.io/marek-mraz-jc/joinedcontext-platform@sha256:1405436f6b9c0b936cb7429d068ea3c77c071fd87bf23c6bb87d392ea3eee03a \
    /usr/local/bin/jcctl /usr/local/bin/jcctl
# The same image is the forge's runner (ADR-N-028): the pod's command is `runner`.
COPY --from=runner --chmod=0755 /gitea-runner /usr/local/bin/gitea-runner
COPY builder/runner.sh /usr/local/bin/runner
COPY builder/runner.yaml /opt/runner/runner.yaml
# The restricted Pod Security Standard runs it as this uid, with nothing it may write but /tmp.
USER 1000:1000
WORKDIR /tmp
ENTRYPOINT ["/usr/local/bin/build-app"]
