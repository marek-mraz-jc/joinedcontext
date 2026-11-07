# Contributing

Antares is developed in the open under EUPL-1.2. This file is the whole
process: where a commit goes, what has to be green before it moves, and
how a release is cut.

## Branches

| Ref | Holds | How it moves | Gate |
|---|---|---|---|
| `dev` | the next release; the default branch | maintainers push commits, everyone else opens a pull request into it | `ci`: workspace tests, then the quick ETSI matrix (file, postgres, timescale) |
| `main` | released code and nothing else | only by a pull request from `dev` | `full`: workspace tests, all seven ETSI cells, both wasm tiers |
| `v*` tag | one release, cut on `main` | a maintainer tags the merge commit | `full` again; artifacts publish only when it is green |

GitHub rulesets (`dev/branch-protection.sh` applies them) and one job
of `full` (`dev/check-source.sh`) enforce the table:

- `main` rejects direct pushes, force pushes and deletion. A pull
  request into it merges only with the `full` checks green, and only as
  a merge commit.
- `dev` rejects force pushes and deletion. History there is linear: a
  pull request lands by rebase or squash, never as a merge commit.
- A pull request into `main` comes from this repository's `dev`. One
  from any other branch fails its first check before the matrix starts:
  retarget it to `dev`.
- A `v*` tag names a commit on `main` whose `Cargo.toml` workspace
  version equals the tag, or the release run stops before it publishes.
  A pushed release tag is never moved and never deleted.

`dev` is the default branch on purpose. A new pull request targets it
without anyone choosing, and the scheduled workflows (`full` twice a
week, `strict`, `fuzz`, `roll-weekly`, `scale-weekly`) run on the
default branch, where the new code is. The `:dev` image and the public
ETSI badges describe `dev`; `:latest` and the versioned images describe
`main`.

## The path of a change

**From a fork (everyone without write access).**

1. Fork, then branch from `dev`: `git switch -c 5.6.6-delete-entity origin/dev`.
2. Commit under the rules below: one change per commit, signed off.
   Before the push, run `dev/preflight.sh`: the per-push gate's checks
   (fmt, the clippy wall, the all-features tests, licence lines, machete,
   the ledger, module rules, the vanilla tree and the docs gates) on the
   crates the branch touches, a few minutes warm. When `antares-sql`, `-api`, `-broker` or `-ops` is touched and
   `ANTARES_TEST_DATABASE_URL` is unset, it starts the PostGIS image of
   `workspace.yml` (PostgreSQL 16, the major dev and prod run) in docker
   for the pg tests. A new CI-only check lands in `dev/preflight.sh` in the same
   change, with a comment naming its CI step.
3. Open the pull request into `dev`. `ci` runs on it; a first-time
   contributor's run waits for a maintainer to approve it.
4. Keep the branch current with `git rebase origin/dev`, not by merging
   `dev` into it. `ARCHITECTURE.md` names each crate and module without
   its size; `python3 dev/spec.py sizes` prints the line counts.
5. A maintainer lands it by rebase when every commit stands on its own,
   by squash when the branch is one change told in several commits.

What a pull request from a fork can and cannot do here:

- Its run waits for a maintainer's approval, every time, and the
  maintainer reads the diff of `.github/` and `dev/` before approving,
  because the run executes the pull request's code.
- It gets a read-only token and no secrets. No workflow uses
  `pull_request_target`, and the jobs that publish an image, a release
  or the Pages site run on this repository's branches and tags only.
- It reads the build caches and never writes one, so it starts warm and
  leaves nothing behind for a later run to pick up.
- The runs that rent hardware (`perf-weekly`, `scale-weekly`) start by
  schedule or dispatch, never from a pull request.
- Nothing a `v*` tag publishes is built from a cache.

**As a maintainer.** Commit on `dev` and push a batch. Every push to
`dev` runs `fast` (the per-push gate: fmt, clippy, the workspace suite
on the default and on all features, the OData ABNF cases, the
repository rules that need no build and gitleaks over the pushed
commits), which is what a review closes on. Everything else of
`workspace.yml` (each feature alone, binary sizes, MSRV, semver, the
shared crates, the wasm32 and fuzz builds, the SBOM, cargo-audit and
cargo-deny) runs in `sweep`: weekly on `dev` when it moved, and on the
commit a production release pins (T-6151). A green `fast` run starts `ci` on the same commit
(`workflow_run`): the ETSI matrix and the arm64 build at once, no second
workspace run, and a green run publishes the `:dev` image. A `fast` run
that is not green starts nothing (its `ci` run cancels itself). A
scheduled or dispatched `ci` run skips the workspace gate when a green
`fast` run already passed it on the same commit, and runs it otherwise;
`with_workspace: true` forces it.
Work that will stay red for more than a day
lives on a topic branch and comes in by pull request like a fork's.

**Promotion to `main`.** When `dev` holds a release:

1. Set the workspace version in `Cargo.toml` on `dev`
   (`release: v0.MINOR.PATCH` as the commit subject), then open a pull
   request `dev` → `main` with the same title.
2. `full` runs when the pull request opens, reopens or leaves draft. A
   later push to `dev` moves the pull request's head without rerunning
   the grid. Red means the fix goes to `dev`; then run `full` once on the
   final head (Actions → full → Run workflow on `dev`, or flip the pull
   request to draft and back). The required checks hang on the head
   commit, so the merge stays blocked until that run is green.
3. Merge with a merge commit. Squash or rebase here would give `main`
   commits `dev` does not have, and the next promotion would conflict
   with itself.
4. Tag the merge commit on `main` and push the tag: `dev/release-tag.sh`
   (`--dry-run` shows the tag and the commit first). It reads the version
   from `origin/main`, refuses a commit that is not the promotion's merge
   or a tag that already exists, signs when git has a signing key and
   annotates otherwise, and pushes on a typed `yes`. The tag run is the release gate, and a tag is final: a bad release
   is fixed by the next PATCH, not by moving the tag.

**Dependency updates.** Dependabot (`.github/dependabot.yml`) opens
pull requests into `dev` every Monday: one grouped pull request of minor
and patch bumps per ecosystem (cargo, GitHub Actions, Docker base
images, the playground's npm packages), a separate one per major bump,
and a security update the day its advisory lands. `ci` runs on each like
on any pull request, and a maintainer merges it by squash after reading
the changelog of anything that parses input or touches the network.
Nothing merges itself: a green matrix proves the broker still conforms,
not that a new upstream release is trustworthy. A cargo bump that moves
a `[workspace.dependencies]` version without its `wasm32` twin fails
`dev/check-wasm-pins.py`; push the matching pin to the same branch. The
weekly `advisories` sweep (`cargo deny`) files an issue for an advisory
no update resolves yet. Updates reach `main` with the next promotion.

**Urgent fixes** take the same road: a commit on `dev`, then a promotion.
There is no hotfix branch, so `main` never holds a change `dev` lacks.

## Build

```bash
cargo build -p antares-broker        # the `antares` binary
cargo test --workspace -j 2          # NOTE: -j 2 — default parallelism
                                     # OOM-kills the linker on small boxes
```

Integration tests that need services are env-gated and skip loudly:
`ANTARES_TEST_DATABASE_URL` (PostGIS), `ANTARES_TEST_NATS_URL` (JetStream),
`ANTARES_TEST_MQTT_URL` (mosquitto).

## The rules that are enforced

- **Spec-first.** Every normative behaviour is implemented from its ETSI
  CIM 009 V1.9.1 clause and the function carries a doc comment citing the
  clause number. The conformance ledger lives in `docs/spec/` (one file per
  clause; `python3 dev/spec.py check` gates format in CI).
- **Test-first.** Write the clause's tests before the implementation and
  watch them fail on the missing behaviour. Every test carries at least
  one negative assertion (what must NOT be in the response).
- **One clause = one commit**, message prefixed with the clause number
  (`5.6.6: …`), committed on a green targeted run (`cargo test -p
  <touched-crate> <filter> -j 2`) plus the clause's Robot TPs green against
  one local memory-store broker (`resources/variables.py` ships upstream's
  compose addresses, so a bare `robot` run overrides them the way
  `dev/etsi-run.sh` does):

  ```bash
  cargo build -q -p antares-broker -j 2
  ANTARES_HTTP_PORT=9377 ./target/debug/antares &
  cd ngsi-ld-test-suite && robot --variable url:http://localhost:9377/ngsi-ld/v1 \
    --variable temporal_api_url:http://localhost:9377/ngsi-ld/v1 \
    --variable notification_server_host:127.0.0.1 \
    --variable context_source_host:127.0.0.1 \
    --variable context_server_host:127.0.0.1 \
    --variable mqtt_broker_host:ngsi-ld-test-suite-mosquitto-container \
    TP/path/to/<tp>.robot
  ```

- **ETSI validation.** `STORE=<mode> dev/etsi-local.sh` runs the suite for
  ONE store mode locally (the one you touched); the CI cell matrix is the
  authority — the quick preset (file, postgres, timescale) runs all ten
  suites per cell on every dispatch, the full seven-cell preset twice a
  week and on `v*` tags.
- `cargo fmt` on touched crates; clippy is a CI wall
  (`unwrap_used`/`expect_used` denied outside tests, `unsafe_code` forbidden).
- **Each feature alone.** A cargo feature has to build, lint and test with
  no other feature on: the `Each feature alone` jobs of `workspace.yml` run
  `cargo hack --each-feature` (one per feature-carrying crate) and are
  required checks. A new feature is
  covered by declaring it in its crate's `[features]`; only a debugging
  hook that cannot build in CI (today `console`) is excluded there, with the
  reason beside it.
- Naming comes from the spec: types verbatim from CIM 009 §5.2, one public
  fn per spec operation; `Manager`/`Service`/`Util`/`Helper` are banned
  suffixes.

## Where things live

See the README's repository-layout table and [docs/README.md](docs/README.md).

## Versioning & releases

Semantic versioning, with the version's meaning defined by these surfaces:
the NGSI-LD API (pinned to ETSI CIM 009 V1.9.1 — spec-versioned, not
ours to break), the `ANTARES_*` environment variables
(docs/src/configuration.md), and the on-disk store formats (redb file
format version, Postgres migrations).

- **Pre-1.0**: `0.MINOR.PATCH` — breaking changes to env vars or store
  formats bump MINOR and are named in the release notes; PATCH is
  fixes/additions.
- **Store-format changes** always ship with a migration note (and for the
  file store, a format-version bump — the broker refuses mismatched files
  rather than guessing).
- **Releases** are `v*` tags on `main`, cut after a promotion (see
  "The path of a change"). The tag triggers the full seven-cell ETSI
  matrix as the release gate plus the examples job; artifacts (multi-arch
  images, binaries, wasm bundle, SBOM) publish only on a green gate, and
  the release notes are the commit subjects since the previous tag.

## Sign-off (DCO)

Contributions use the [Developer Certificate of Origin](https://developercertificate.org/):
add `Signed-off-by: Your Name <email>` to each commit (`git commit -s`).
No CLA — the DCO plus the EUPL-1.2 inbound=outbound rule is the whole
agreement.
