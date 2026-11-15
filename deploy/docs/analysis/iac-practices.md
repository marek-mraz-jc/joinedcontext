# Config as code: rules and gaps

The rule set this repository follows for infrastructure and configuration as code, each rule
traced to the source it comes from, and the gap table against this repository at the commit
named below. Task T-4953 (group iac-practices); the fixes are T-4954 (pinning, renovate, tool
versions), T-4955 (versioning, releases, rollback) and T-4956 (reproducible render, drift,
values schemas, validation chain).

## 1. Sources read

| Source | Version read | What it gave |
|---|---|---|
| helmfile `docs/` (writing-helmfile, environments, values-and-merging, configuration, cli, advanced-features), `examples/`, `test/` | tag `v1.8.1` (226c16b), the version CI and the image run | layering, environments, lock file, diff flags, `helmDefaults`, `missingFileHandler` |
| helmfile/helmfile-action `action.yaml` | 1f44bcb (v2.4.7, the SHA ci.yml pins) | `helm-version` and `helm-plugins` default to `latest` |
| Helm docs, `docs/chart_best_practices/` and `docs/topics/charts.mdx` | helm-www b2e8b8c (2026-10-20); Helm `v4.3.0` source | image pins, dependency versions, values naming and documentation, values.schema.json |
| fluxcd/flux2-kustomize-helm-example | 4f1a5b1 | base + per-environment overlay that holds only the difference; CI validates every manifest against schemas and refuses deprecated APIs |
| kubernetes/k8s.io (the Kubernetes project's own infrastructure) | 4df2be2 | exact chart versions in the file (`version: 3.244.0`, `targetRevision: v1.20.2`); `hack/verify-*.sh` run by one `hack/verify.sh` (conftest, yamllint, shellcheck, manifests); a bot opens bump PRs |
| this repository | origin/main 8838453f (2026-10-21) | our state |

The clones were throwaway (`/home/dev/tmp-iac`) and are deleted; no secret was read.

## 2. Rules

Owner rule (2026-10-21): config as code behaves like helmfiles. Declarative files are the only
source; exact versions and digests sit in those files; one command renders, diffs and applies an
environment; every change passes validation before any apply, and CI fails when a step is skipped.

- **R1 Exact pins in the file a reader sees.** Every remote chart has an exact `X.Y.Z` version
  and every image a digest, in the helmfile or values files. Source: Helm pods.md "a fixed tag or
  the SHA of the image"; k8s.io pins charts exactly. Helm's dependencies.mdx recommends `~1.2.3`
  ranges for a chart's own dependencies; we do not follow it, see R2.
- **R2 No lock file.** helmfile offers `helmfile deps` and `helmfile.lock` (cli.md "deps",
  advanced-features.md "Lockfile per environment") to freeze ranges. With R1 there is no range to
  freeze, so the lock only adds a second file to keep in sync. Decided (worker-t4953, owner may
  override): no `helmfile.lock` and no `Chart.lock`, because the owner asked for none and none of
  our 26 local charts declares `dependencies:`; a chart that later needs a dependency pins it
  exactly in `Chart.yaml` and commits the `Chart.lock` Helm writes (Helm needs it to build the
  chart; it holds nothing the `Chart.yaml` does not). Options: helmfile.lock per environment
  (rejected: generated state), version ranges + lock (rejected: the reader sees a range, not what runs).
- **R3 Tool versions in one file.** helm, helmfile, helm-diff, kubeconform, conftest, kyverno,
  gitleaks, sops and the Kubernetes schema version are read by CI and `just` from one file, with
  checksums. Source: helmfile-action defaults `helm-version: latest`, which makes a render depend
  on the day it ran.
- **R4 Update bot with a cooldown.** Renovate proposes every bump (charts, images by digest,
  GitHub Actions by SHA, tools in the R3 file) no earlier than 7 days after release (owner rule,
  sandbox guide). Source: k8s.io autobump; renovate `minimumReleaseAge`.
- **R5 One base, overlays hold only the difference.** An environment file states what differs
  from the defaults; it never copies the defaults. Source: helmfile environments.md
  ("make your configuration DRY"), writing-helmfile.md layering; the flux example's
  `apps/base` + `apps/<env>` patch.
- **R6 A schema per values surface.** Each local chart ships `values.schema.json` (Helm refuses a
  value that fails it on install, lint and template); each component's environment values are
  checked against a schema in CI. Source: Helm topics/charts.mdx "Schema Files"; values.md
  "Document values.yaml".
- **R7 Fail on a missing file or key.** `missingFileHandler: Error` (helmfile v1 default) and no
  `default` on a key that must exist. Source: writing-helmfile.md "Missing keys and Default values".
- **R8 Validation chain before any apply, every environment.** helmfile lint (helm lint per
  chart), helmfile template, kubeconform `-strict` with CRD schemas, conftest and kyverno, secret
  scan of the render, golden render. Source: flux example `test.yaml`; k8s.io `hack/verify.sh`.
- **R9 Same commit, same render.** A golden render per environment in CI; any change to it is a
  reviewed diff in the same branch.
- **R10 Drift is measured.** `helmfile diff --detailed-exitcode --suppress-secrets` against each
  live cluster on a schedule; a non-empty diff becomes a task. Source: cli.md "diff",
  helm-diff `--detailed-exitcode`.
- **R11 Secrets.** sops + age for encrypted files, `secretRef` in manifests, nothing in plaintext
  in Git or a render. Source: environments.md "Environment Secrets"; platform rule.
- **R12 Versioned releases.** Semver tags, release notes generated from the commits, the running
  version recorded in the cluster, prod applies only a tag, rollback applies the previous tag.
  Source: Helm conventions.md (SemVer 2); helmfile cli.md "deps" names the goal ("untested chart
  versions won't suddenly get deployed to the production environment").
- **R13 helmDefaults state the failure behaviour.** `wait`, `waitForJobs`, `timeout`, and a
  decided `historyMax` and rollback policy. Source: configuration.md `helmDefaults`.
- **R14 Policy as code at admission and in CI.** The same rules (Kyverno at admission, conftest
  on the render) run in both places. Source: k8s.io `policy/` + verify-conftest.sh.
- **R15 Layout and docs.** One folder per component with its chart pins, image pins, values and a
  README; names follow DNS-1123 (Helm conventions.md).

## 3. Gap table

| Rule | Our state at 8838453f | Gap | Fixed by |
|---|---|---|---|
| R1 | 12 of 12 remote chart versions exact in `components/*/charts.yaml`; 48 of 48 images carry a digest in `components/*/images.yaml`; ci.yml `check-image-digests.py` gates the render | none | no change: holds; T-4954 keeps it gated |
| R2 | no `helmfile.lock`, no `Chart.lock`; no local chart has `dependencies:` | none | no change (Decided above) |
| R3 | helmfile `v1.8.1` written twice in ci.yml; helm and helm-diff unpinned (`latest` via helmfile-action); ci.yml render job downloads kubeconform `releases/latest` without a checksum; conftest and gitleaks in ci.yml `env:`; kyverno and kubeconform in reusable workflow inputs | render depends on the run day; kubeconform unverified download | T-4954 |
| R4 | `renovate.json` is the GitLab-era file: `baseBranchPatterns: staging`, `gitlabci` manager, no `github-actions` manager, no `minimumReleaseAge`; `pin-images.yml` bumps our own image digests | third-party charts, images and actions get no bump PRs on `main`; no cooldown | T-4954 |
| R5 | `ci/example-deployments/environments/dev` and `smartcity` repeat most lines: ckan 137/144, global 123/141 identical, gitea, gitea-runner, node-storage, pipeline-runner, wasm-host fully identical | copies drift apart; a fix lands in one environment only | T-4956 |
| R6 | `component.schema.json` checks `component.yaml`; `keycloak-clients.schema.json` one surface; 0 of 26 local charts has `values.schema.json`; environment values unchecked | a typo in a values key renders silently | T-4956 |
| R7 | no `missingFileHandler` override (v1 default Error); templates read optional keys with `get` | none | no change |
| R8 | ci.yml per environment: template, kubeconform `-strict -ignore-missing-schemas`, digest check, conftest, secret scans; kyverno apply in tests job; no `helm lint`/`helmfile lint` anywhere | CRs skip schema validation; 26 local charts never linted | T-4956 |
| R9 | `ci/golden/local.txt` lists the objects of `local` only | dev, smartcity, production renders can change unreviewed | T-4956 |
| R10 | `scripts/validate-dev.py` runs `helmfile diff --detailed-exitcode --suppress-secrets` on demand (`just dev-validate`); nothing scheduled; prod never diffed | drift found only when someone looks | T-4956 |
| R11 | `.sops.yaml` (age), `tests/test_sops_recipients.py`, `check-secrets.py`, gitleaks on history and on renders; the owner's offline age key is still to be added | owner key pending (owner action) | no change: tracked in `.sops.yaml` comment (T-1712) |
| R12 | tags `v0.1.0` and `prod-20261019T0409Z` only; `releases/v0.1.0.md` + `next.yaml` and `just release-notes`; `scripts/upgrade.py` reads `jc-release` ConfigMap and rolls back on red; `just rollback <prod-*>` moves the `release` branch; `just prod-apply` syncs the working tree, not a tag; no CHANGELOG | prod can run an untagged tree; releases not cut | T-4955 |
| R13 | `defaults/helm-defaults.yaml`: wait, waitForJobs, timeout 300; no historyMax (Helm default 10); no atomic | rollback lives in upgrade.py, not helm; fine for one node | no change: Decided (worker-t4953, owner may override): keep rollback in upgrade.py, because `atomic` rolls back on any slow rollout on the 16 GB node and hides the cause; upgrade.py rolls back after a red smoke instead |
| R14 | `policies/*.rego` via conftest in CI; Kyverno policies at admission with `kyverno test` and `kyverno apply` on renders in CI | none | no change |
| R15 | 33 components, 31 with a README (`template-component` + `copier.yaml`); `charts.yaml`, `images.yaml`, `values/`, `helmfile.yaml.gotmpl` per component | apps-db and wasm-host have no README | T-4956 (adds the README check with the schema check) |
| actions | 76 of 76 `uses:` pinned to a full SHA with a version comment | none | no change |

## 4. Other repositories

- **platform**: R3 holds through `Cargo.lock` and SHA-pinned actions (63/63); its `renovate.json`
  has no `minimumReleaseAge` (R4). Tags exist per crate (`jc-core-v0.7.76`). Fixed by:
  tasks/later T-4963 (renovate cooldown in platform, portal, conformance).
- **portal**: actions SHA-pinned (68/68); jc-core pinned by exact git rev; `renovate.json` without
  cooldown (later T-4963); no release tags, images go by digest through deployment pins, so R12
  is met at the deployment layer: no change.
- **docs**: `Deployment/06-updating.md` and `13-configuration-reference.md` describe the update and
  config surface; they gain the R12 release flow when T-4955 lands (docs half after code half).
- **conformance**: actions SHA-pinned (67/67); `renovate.json` without cooldown (later T-4963);
  suites run pinned images by digest, R1 holds.

## 5. Owner additions mapped

- No lock file: R2.
- Behave like helmfiles: one command per environment exists (`just dev-apply`, `just prod-apply`,
  `just upgrade`); validation chain R3, R6, R8, R9 closes the remaining steps (T-4954, T-4956);
  a reviewed diff before apply is R10 plus `upgrade.py`'s diff step (T-4955 makes prod apply a tag).

## 6. How R1, R3 and R4 hold (T-4954)

- R1, R2: `scripts/ci/check-chart-pins.py` reads `helmfile list --output json` of every
  environment, the BBSK demo and `ci/kyverno` in ci.yml and fails on a remote chart without one
  exact `X.Y.Z` version; `--no-lock .` fails on any `helmfile.lock`. `check-image-digests.py`
  keeps failing on a tag-only image. `tests/test_pins.py` plants both.
- R3: [tools.env](../../tools.env) holds every tool version and the sha256 of every download;
  `.github/actions/tools` loads it and installs helmfile, helm and helm-diff at those versions.
  kubeconform now validates against the cluster's Kubernetes version (1.33.7) instead of the
  schema repository's `master`.
- R4: `renovate.json` targets `main` with `minimumReleaseAge: 7 days` and four managers this
  repository has: the regex managers (charts.yaml, images.yaml with tag + digest, tools.env),
  `helmfile`, `dockerfile`, `github-actions`. Minor, patch and digest bumps come grouped per
  kind (helm charts, container images, github actions, ci tools); a major is its own PR. Our own
  `ghcr.io/marek-mraz-jc` images stay with `pin-images.yml`. ci.yml's `renovate` job runs
  `renovate-config-validator --strict` at the pinned Renovate version.
- Decided (worker-t4954, owner may override): no `helm-values` manager, because no values file
  in this repository names an image (each local chart's `image:` is empty and filled from
  `components/*/images.yaml`); an enabled manager with nothing to read is noise. A tools.env
  bump PR leaves the checksum stale and fails CI until a reviewer pins the new sha256, because
  Renovate cannot compute a release asset's checksum; options were the release's own checksum
  file (rejected: it trusts the same release it checks) or no checksum (rejected: R3).

## 7. Closed by T-4956

- R5: `environments/_layers/hosted/` holds the five value files dev and smartcity shared byte for
  byte; each names the layer in `environmentLayers` (`_layers/README.md`). ckan and global differ
  by domain and stay per environment: an environment file cannot read `global.domain`.
- R6: each of the 29 local charts has a strict `values.schema.json` (`additionalProperties:
  false`); Helm enforces it on every template and sync. `tests/test_values_schemas.py` plants a
  typo per chart.
- R8: ci.yml runs `helm lint` on every local chart and kubeconform without
  `-ignore-missing-schemas`, custom resources against the CRDs catalog at `CRDS_CATALOG_COMMIT` in tools.env.
  `helmfile lint` is left out: it repeats the render with the same schemas and needs every
  remote chart again.
- R9: `ci/golden/<env>.txt` for the nine environments in `ci/golden/ENVIRONMENTS` plus the
  BBSK demo; smartcity (production) is rendered in CI for the first time.
- R10: `just drift dev|smartcity` runs `helmfile diff --three-way-merge` (live objects, so a
  `kubectl edit` shows) under a 30-minute token of `jc-drift-reader` (get and list only) and
  refuses to run when that identity can write; one result per release for `tasks/file-failures`.
- R15: apps-db and wasm-host have a README; the test asks every component for one.

## Related

- joinedcontext-docs `Deployment/06-updating.md` (the update procedure)
- [scripts/upgrade.py](../../scripts/upgrade.py), [scripts/validate-dev.py](../../scripts/validate-dev.py)
- [renovate.json](../../renovate.json), [.github/workflows/ci.yml](../../.github/workflows/ci.yml), [tools.env](../../tools.env)
