# joinedcontext-portal

The Portal is the one application that manages a joinedcontext installation: a Rust backend
and a React UI in one repository, one image and one route. Everything a person, a script, the
assistant or an MCP client can do here is the same operation behind four doors — there is no
second implementation of "propose this change" for the API and for the button.

## 1. What is in it

```text
joinedcontext-portal/
├── Cargo.toml       the axum application: the Portal API under /api/v1, the operation registry,
├── src/             drafts and workspaces, ServiceAccounts and API keys, the in-process
│                    reconciler (the jcctl crate as a library) and the configuration MCP at
│                    /api/v1/mcp. It serves the built UI from ui/dist at /
├── ui/              Vite + React 19 + TypeScript: the Portal UI, the LinkML editor, the
│                    dashboards (MapLibre + deck.gl), the apps kit and four locales
│                    (en, sk, cs, de). The API client is generated from the backend's OpenAPI
└── sdk/             @joinedcontext/sdk: the package a generated application is built on
```

## 2. How it fits

A person signs in at the edge and reaches the Portal; the Portal writes nothing to a broker
and nothing to a cluster. Every write is a manifest proposed as a merge request in the
organization's Git repository, and the reconciler applies it once it is approved. That is why
the Portal can be restarted, rebuilt or rolled back without losing anything that matters.
Start at
[Architecture/09 — Portal](https://github.com/marek-mraz/joinedcontext-docs/blob/main/Architecture/09-portal.md),
then
[Architecture/06 — Configuration as Code](https://github.com/marek-mraz/joinedcontext-docs/blob/main/Architecture/06-configuration-as-code.md)
for what happens to a change after the button.

## 3. Build

The binary embeds `ui/dist`, so the UI is built first — and the UI links the SDK by
`link:../sdk`, which installs nothing for the sibling, so the SDK is built before that:

```bash
cd sdk && pnpm install --frozen-lockfile && pnpm build
cd ui && pnpm install --frozen-lockfile && pnpm build
cargo build --locked --release
```

## 4. Test

The fast checks, which are the ones the merge gate runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --test openapi_tests
```

And the UI's own, from `ui/`:

```bash
cd ui && pnpm lint && pnpm exec tsc -b && pnpm test -- --run
```

`cargo test --lib --bins` does not run an integration test, and each one is its own binary.
A change to a response shape needs the whole crate. Its apps-database tests need a PostgreSQL 16
whose user may create roles and databases, and fail without one rather than skip (AP-149):

```bash
docker run -d --rm --name jc-apps-db -e POSTGRES_PASSWORD=pw -p 5434:5432 postgres:16
until docker exec jc-apps-db pg_isready -h 127.0.0.1 -U postgres; do sleep 1; done
JC_APPS_DB_TEST_URL=postgres://postgres:pw@127.0.0.1:5434/postgres cargo test -p joinedcontext-portal
```

`pnpm exec tsc -b` rather than `tsc -p`: the project build covers `ui/tests` as well, and a
new schema field breaks the contract suites before it breaks the test you wrote.

## 5. Run locally

The Portal needs a database, a realm and the forge it proposes changes to:

```bash
JC_PORTAL_DATABASE_URL=postgres://localhost/portal \
JC_PORTAL_ORG_DOMAIN=example.org \
JC_OIDC_ISSUER=https://idm.example.org/realms/joinedcontext \
JC_GITEA_URL=http://127.0.0.1:3000 \
JC_GITEA_OWNER=my-organization \
JC_GITEA_REPO=organization \
cargo run
```

It then serves the UI and `/api/v1` on `0.0.0.0:8080` (`JC_PORTAL_BIND` moves it). The UI
alone, against a Portal that is already running, is `cd ui && pnpm dev`. The whole list of
variables the binary reads is in `src/config.rs`, each with the sentence that says what it is
for.

## 6. Security

Report a vulnerability privately through this repository's GitHub security advisories, or by
the contact in [SECURITY.md](SECURITY.md). Please do not open a public issue for one.

Three things are worth knowing before you read the code. Every route's permission is decided
by the operation it runs, not by the handler, so a new route inherits the rule of its
operation. A secret is never stored, logged or returned — it is named by a `secretRef` and
resolved by the reconciler. And the UI renders no HTML it did not write: what comes back from
the API is text.

## 7. Working here

Read the owning chapter and requirement family before writing code, cite the requirement ids
in the commit message, and ship the tests with the change. A UI change ships its own test in
`ui/tests/`, and a control a caller may not use is refused with a reason rather than removed
from the page.
