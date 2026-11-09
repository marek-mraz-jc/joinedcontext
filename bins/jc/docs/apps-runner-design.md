# App builds in jc: design

Written 2026-11-09 by jc-apps-design, from monorepo `origin/jc-portal-1` 3bf19d23, deployment
`origin/main` 11d32d09d, portal `main` bcb006399 and platform `main`. Paths without a prefix are
in the monorepo. Other prefixes: **D** = joinedcontext-deployment, **P** = joinedcontext-portal
(the copy dev runs), **PL** = joinedcontext-platform. The Portal code is the same in P and in
`crates/portal`.

## 0. What fails today, on the real dev export

Run `/workspace/evidence/jc-migrate/smoke/charts-apps/2026-11-09T025813Z/dev` (jc 4fdfd542,
input `/workspace/evidence/jc-rehearsal/export/dev`, 80 App repositories, 44 Apps):

| check | count | result |
|---|---|---|
| `app.portal`: `GET /api/v1/projects/{p}/apps/{name}` | 44 | pass (200) |
| `app.served`: `GET /apps/{name}/` | 44 | skip: 39 answer 404, 5 answer 403 |
| `app.page` (browser) | 44 | skip: not served |
| `app.repo`: `GET /git/api/v1/projects/joinedcontext-apps/repositories/{repo}` | 80 | skip: "no shako-token in --state", so this run checked nothing |

The 44 Apps fall into four classes. Each class fails for a different reason, and only the last
two need a build runner.

| class | Apps | `GET /apps/{name}/` | why |
|---|---|---|---|
| A. shipped with the Portal image (`joinedcontext.com/shipped-with: portal`, `spec.source.path`, no `git`) | 27, all `ui`: banskabystrica ×5, bbsk ×6, praha ×4, zilina ×4, sharing ×5, helsinki-data, helsinki-kartta, bike-stations | 404 (403 for `visibility: roles`: banskabystrica-skoly, bbsk-mosty, praha-odpad, sharing-roles) | The Portal image builds these bundles into `/srv/apps` (`crates/portal/Dockerfile:54-190`) and serves them from `JC_PORTAL_APPS_DIR` (`static_host.rs:552-575`). `bins/jc/Dockerfile` has no such stage, and jc sets no `JC_PORTAL_APPS_DIR`. With the key unset, every `/apps/*` path answers 404 (`config.rs:221-224`). **Needs no runner.** |
| B. own repository, `ui` (`status.build` digest committed) | 4: alerts-desk, helsinki-alerts (roles: 403), helsinki-bikes, helsinki-events | 404 / 403 | `status.build` names a build. The Portal fetches it from the forge's generic package `app-{name}` (`apps/fetch.rs:49-97`), but jc sets no `JC_PORTAL_APPS_CACHE_DIR`, and the export did not carry dev's packages. `build_missing` would say `BuildMissing` once the cache dir is set (`static_host.rs:600-623`). **Needs dev's built packages, or a rebuild.** |
| B'. own repository, `ui`, never built | 1: t6429-bike-live | 404 | No `status.build`, so `report_unbuilt` sets `Pending`/`NoBuild` (`static_host.rs:625-664`, logged by `reconciler/daemon.rs:2193-2201`). **Needs a build.** |
| C. own repository, `wasm` | 10: air-weather-explorer, alerts-heatmap, bike-rebalancing, bike-weather-demand, data-quality-inspector, district-compare, event-day-planner, kpi-forecast, news-topics, transit-reach | 404 | The same as B for the static half. The `/api/*` half also needs the wasm host, the artifact store (`components/sha256-<hex>.wasm`) and apps-db. jc wires none of these: no `JC_PORTAL_ARTIFACT_STORE_*`, `JC_PORTAL_APPS_DB_URL` or `JC_PORTAL_WASM_*` key (`config.rs:471-548`). `bins/jc/CUTOVER.md:23` keeps `wasm-host` and `apps-db` outside jc. |
| D. own repository, `ui-rust` | 2: air-quality, hsl-transport | 404 | The build is an OCI image in the forge's registry, run as Service `app-{name}` in the project namespace (`reconciler/edge_file.rs:166-175,758-771`). That needs Kubernetes. A standalone jc cannot run it. |

Three side findings:
- The 403s are correct behaviour. A `visibility: roles` App refuses a caller who lacks its role before the root lookup (`static_host.rs:78-106,144-157`). After the fix these Apps stay 403 for the suite's person. The suite should sign in as a role holder, or count 403 on a roles App as "kept out".
- `repositoryUrl` reads `…/git/user/oauth2/keycloak?redirect_to=%2Fgit%2Fjc-apps%2F…`, which is Gitea's sign-in path (`forge/shako.rs:700-724`). shako-web only redirects there (`crates/shako/web/src/lib.rs:768-797`), so the link works. It is cosmetic.
- jc names the apps owner `jc-apps` (`bins/jc/src/setup.rs:31`). Dev names it `joinedcontext-apps` (D `_layers/hosted/gitea.yaml.gotmpl:9`). The App binds its repository by `{project}_{app}`, so the import works under either owner. Any carried package must also go under `jc-apps`.

## 1. What dev does today

### 1.1 Who builds, and where

1. **The workflow file.** Each App repository holds `.gitea/workflows/build.yml`. The Portal writes it in the first commit and refuses any later change under `.gitea/` (P `src/agents/repository.rs:19-27,52-53`). Only three texts exist: `sdk/template`, `sdk/template-fullstack` and `sdk/template-wasm`. The export's 24 workflow files have three md5 sums, 12 + 10 + 2. Triggers are a push to `main` and `workflow_dispatch` (the Portal's Rebuild, AP-103).
2. **Pickup.** Every 5 s the Portal lists queued jobs with `GET /orgs/{owner}/actions/jobs?status=queued` (P `src/git/gitea.rs:1334-1340`; `apps/build_pods.rs:350-401`). For an `app-build-node` or `app-build-rust` job of a live App's repository, it mints a registration token for that repository alone (`build_pods.rs:406-437`). It then creates Kubernetes Job `build-{app}-{id}` in the runner namespace, with the token in a Secret the Job owns (`build_pods.rs:119-299`).
3. **The build pod.**
   - The image is `joinedcontext-app-builder` or `joinedcontext-app-builder-rust`, by digest (D `components/gitea-runner/images.yaml:11-21`).
   - It runs act_runner in host mode, with `GITEA_RUNNER_ONCE=1` and `--ephemeral` (`builder/runner.sh:24-57`). The registration token is read once and deleted, `/tmp/runner` is wiped, and stray processes are killed.
4. **Build steps** (`builder/build-app.sh`):
   - It fetches the commit with the job token sent as a header (`:35-40`). App code runs under `env -u JC_FORGE_TOKEN -u ACTIONS_RUNTIME_TOKEN` (`:42-45`). The comment there admits the same uid can still read `/proc/<pid>/environ`.
   - **No install.** The template's `node_modules` are symlinked in from the image (`:54-64`). Cargo runs `--offline --locked` over the image's crate store (`build-wasm.sh:32-48`).
   - `ui`: vitest, tsc, vite build, and `build-wasm` when `wasm/Cargo.toml` exists.
   - `wasm`: `build-component` targets `wasm32-wasip2` and writes `.jc/component.wasm` (`build-component.sh:76-98`).
   - `ui-rust`: a musl binary, then an OCI layout `image.tar` (`build-app.sh:76-120`).
   - Every kind runs the browser checks, then writes the sha256 digest and `build.json` (`:162-184`).
5. **Upload.** `lane.mjs upload` uses the forge's Actions artifact API v4 with `ACTIONS_RUNTIME_TOKEN`, which is scoped to one run (`builder/lane.mjs:658-702`). It uploads `bundle-{commit}` or `image-{commit}`, plus `sbom-{commit}` and `report-{commit}`.
6. **Propose.** A second job, `propose`, runs on the shared runner (`node-22` / `rust-1.90`). It runs no App code. It holds `JC_LANE_TOKEN`, a Keycloak client `jc-build-lane` token (900 s) that a CronJob refreshes every 5 min as an org Actions secret (D `components/gitea-runner/keycloak-clients.yaml:11-39`). Its one right is Role `build-lane`, verb `propose` on `App` `status.build` (D `components/context-gateway/seed/helsinki/helsinki-role-build-lane.yaml:6-14`). That right covers **every** App. The job calls `PUT /api/v1/projects/{p}/apps/{app}`, first with `?dryRun=All`, and retries a 409 11 times (`lane.mjs:728-775`).
7. **Check and publish** (`api/mutate.rs:1228-1243` → `apps/built.rs:151-298`). The Portal checks four things: the commit is the head of the default branch; the artifact of that name came from a run of that commit; it is under 64 MiB (128 MiB for an image); its sha256 equals the proposed digest, and the sbom came from the same run. It then publishes with its own forge token:
   - `ui` and `wasm` go to generic package `app-{name}`, version `{commit}-{digest12}`;
   - `ui-rust` goes to the forge's OCI registry as `app-{name}:{commit}`;
   - for `wasm`, the component also goes to S3 `components/sha256-<hex>.wasm`. The App gets a shard, and its apps-db schema is migrated (`apps/wasm_apps.rs:119-154`).
8. **Serve.** On each sync, every Portal replica fetches the package, checks its digest and unpacks it into `JC_PORTAL_APPS_CACHE_DIR/{name}/{hex}/` (`apps/fetch.rs`). The static host serves `/apps/{name}/` from there, with `integrity.json` SRI and a per-App CSP, on the App's own origin `{name}.apps.{domain}` (`static_host.rs:844-878`).
   - For `wasm`, the edge sends `/api/*` to `jc-wasm-host-{shard}` (`edge_file.rs:571-575,779-783`).
   - For `ui-rust`, `/*` goes to Service `app-{name}`.

### 1.2 Sandbox, egress, secrets, limits

| property | dev today | file |
|---|---|---|
| container | non-root uid 1000, `runAsNonRoot`, readOnlyRootFilesystem, drop ALL caps, seccomp RuntimeDefault, no ServiceAccount token, no service links | D `defaults/environment/security.yaml.gotmpl:2-14`; `build_pods.rs:175-207` |
| runtime | host mode: steps run in the runner container. No docker.sock, no privileged mode, no host mounts, no gVisor or kata | D `values/runner/base-values.yaml.gotmpl:73` |
| egress | gitea-forge TCP 3000, Portal TCP 8080, kube-dns 53. With Linkerd, also 4143 and the control plane. No internet and no registry | D `components/gitea-runner/networkpolicies.yaml:6-41`, `networkpolicies-linkerd.yaml` |
| ingress | none | same |
| secrets in the build pod | one per-repository registration token (init container → 1 Mi memory emptyDir, read once). The job token is scoped to the repository. `ACTIONS_RUNTIME_TOKEN` is scoped to the run. `JC_LANE_TOKEN` reaches the `propose` job only | `build_pods.rs:209-230`; D `components/gitea/charts/bootstrap/templates/job.yaml:513-523,750-771` |
| time | 20 min (node), 30 min (rust): `activeDeadlineSeconds`, `backoffLimit 0` | `build_pods.rs:87-92` |
| CPU / memory | node 250m/512Mi → 2/2Gi; rust 250m/1536Mi → 2/4Gi | `build_pods.rs:94-105` |
| disk | `/tmp` is an `emptyDir {}` with **no size limit**; per-App cache PVC `build-cache-{app}`, 1 Gi | `build_pods.rs:249`, `:127-162` |
| concurrency | `MAX_RUNNING = 1` per cluster; every runner `capacity: 1` | `build_pods.rs:43-45` |
| artifact caps | bundle 64 MiB, image 128 MiB, component 64 MiB | `built.rs`, `wasm_apps.rs:15-18` |
| Portal RBAC in the runner namespace | Jobs and PVCs: get, list, create, patch, delete. Secrets: create and patch only. No pods, exec or logs | D `charts/build-pods/templates/rbac.yaml:25-33` |
| wasm host | memory 64 MiB, wall time 5 s per request, fuel 2e11, bodies 2 MiB in and 8 MiB out, a job 60 s; per-shard store key and DB password; data under `apps/<shard>/<id>/` and one schema per App | PL `crates/wasm-host/src/limits.rs:35-42`, `jobs.rs:18`, `blob.rs:1-4` |

Every limit above is per deployment. None is per organization.

## 2. Options

jc's current state:
- shako serves no Actions at all. `api/src/compat.rs:64-74` answers rows 6-11 with 404, and S16/S17 are "out of scope, run 4" (`crates/shako/STATUS.md:11-17`).
- shako does serve a generic package registry and OCI `/v2/` with cosign (`crates/shako/registry`).
- The Portal's build-pod dispatcher, check-and-publish and fetch code are all present in `crates/portal`.

| option | what it is | cost | risk | where the wasm host runs |
|---|---|---|---|---|
| **a. ekiben-like build module in jc** | a `bins/jc/src/modules/builder.rs` that runs `build-app` as a child of the jc process | jc's distroless image gains Node, pnpm, Rust 1.90 with three targets, the crate store and Chromium (several GB). jc has no `Command::new` today (`STATUS.md:578-579`) | **High.** App code (vitest, `build.rs`, proc macros, vite plugins) would run in jc's container. That container holds every module's secrets on disk and in `/proc`, and shares jc's memory and CPU. Fencing it needs user namespaces or a jail inside a distroless image. This breaks dev's sandbox, so it is rejected | outside, as today |
| **b. build at publish time, in-process** | the propose/publish request builds before it answers | same as a, plus a request lasting 2-30 min | Same as a, plus a publish that times out at the edge's 60 s. Rejected | outside |
| **c1. keep Gitea and gitea-runner beside jc** | App repositories stay on Gitea. Only the config repository moves to shako | low code; deployment: Gitea's pool of 3, its pod, its Keycloak client and its bootstrap Job stay | Two forges, and the Portal needs two forge clients, while jc-shako-odata removes shako's Gitea API. App repositories never leave Gitea, so `CUTOVER.md` step 4 never ends. The rehearsal cannot run without a second forge | outside |
| **c2. shako-native Actions (S16/S17) for the unchanged act_runner** | shako implements the runner protocol (`runner.v1.RunnerService` Register, Declare, FetchTask, UpdateTask, UpdateLog), workflow YAML with `${{ }}`, `needs.outputs`, secrets, the artifact API v4 and actions runs/jobs/dispatch | large: a workflow engine and an expression language. My estimate is 3-5k lines across shako and its tests, for exactly 3 workflow texts the Portal already refuses to vary | Medium. A new generic code-execution surface in the forge, and a second secret store (Actions secrets). Everything after it is reused unchanged: the build pods, lane.mjs, check_and_publish | outside |
| **d. Portal-driven fixed build lane** (recommended) | Keep the build pod, the builder image, `build-app.sh`, `check_and_publish`, the package/OCI publish and fetch. Replace only the transport. The Portal starts a build itself: a published App whose default-branch head ≠ `status.build.commit`, or a Rebuild. The pod gets one per-build token. It pulls the commit's source from the Portal and puts the artifact back to the Portal. The Portal computes the digest, runs the same checks and writes `status.build` itself | small: an estimated ~600-900 lines. Portal: a build table, three internal routes, the trigger and the reuse of `build_pods.rs`. Builder: `lane.mjs` gains a `jc` mode for fetch and put (~100 lines). No shako change | Low. App code runs in the same pod spec as dev. The pod holds one token less than dev, and the org-wide `JC_LANE_TOKEN` goes away | outside: the same `jc-wasm-host` binary and chart as dev |
| **e. carry dev's builds across** (no runner; a step before d, not an alternative to it) | The export reads dev's generic packages `app-{name}` and images `app-{name}`. The import puts them into shako under `jc-apps`. The committed `status.build` digest already pins each one, and `fetch.rs` checks it | ~150 lines in jc-migrate's export and import | Low: the digest is checked at fetch time. It covers class B and C only up to the last dev build; a new commit still needs d | outside |
| **f. ship the 27 bundles in the jc image** (no runner) | copy the Portal Dockerfile's `apps` stage into `bins/jc/Dockerfile` and set `JC_PORTAL_APPS_DIR=/srv/apps` | ~40 Dockerfile lines and 1 wiring line | None new. It is the Portal image's own path. The image grows by the bundles, an estimated ~10-20 MB, measured in slice 1 | n/a |

## 3. Recommendation

**f, then e, then d.**

f and e make 41 of the 44 Apps servable with no runner at all. That count is the 27 shipped + 4 ui + 10 wasm static halves. The wasm `/api` halves also need the wasm host beside jc. d then builds every new commit. It reuses everything dev runs except the Actions transport.

The reason for d over c2: the Portal already decides what a build is (three fixed workflows, the refusal of `.gitea/` edits), who builds (`build_pods.rs`) and whether the result is good (`built.rs`). Gitea Actions only carried messages between those parts. Building a workflow engine into shako to carry three fixed messages is a new mechanism. d deletes one: the org-wide lane token and the second `propose` job.

If the owner later wants arbitrary workflows (CI for App repositories), c2 can come then. d does not block it.

Placement in d:
- **Kubernetes (dev, stage, prod):** the Portal module inside jc creates the same Job as `build_pods.rs` today, in the runner namespace, with the same Role.
- **Standalone jc (rehearsal, compose):** a slice-5 `jc-builder` container. It takes one build, runs it and exits, and `restart: always` brings it back fresh. This is the same "once" model as `GITEA_RUNNER_ONCE`, with no docker.sock.
- **The wasm host** stays the separate `jc-wasm-host` (D `components/wasm-host`, unchanged limits), beside jc. It is not a jc module: a wasmtime fault, or N × 64 MiB instances, must not take down the forge, the IdP and the broker together.

### 3.1 What stays identical to dev

- **Build sandbox:**
  - the same pod spec: uid 1000, non-root, read-only root, no caps, seccomp, no SA token, no service links, `backoffLimit 0`;
  - the same images by digest and the same `build-app.sh`: no install, cargo `--offline --locked`;
  - the same per-App cache PVC and the same deadlines (20/30 min) and resources;
  - the same `MAX_RUNNING = 1` per deployment.
  - New: an `ephemeral-storage` limit on `/tmp`, sized from slice 4's measurement (dev has none).
- **Egress:** the build pod reaches **jc's Service port only**, plus DNS. That is narrower than dev, which also allowed the forge on 3000: in jc the Portal serves the source, so the pod needs no forge address. There is no internet and no registry.
- **Per-App secrets:**
  - the pod holds one random 256-bit build token, bound to (App, commit, build id, kind) and valid until the deadline + 5 min;
  - the Portal keeps only the token's SHA-256 and compares it in constant time;
  - the token admits only `GET /internal/app-builds/{id}/source.tar` (that commit of that repository), `PUT .../artifact/{bundle|image|sbom|report}` and `PUT .../log`.
  - No forge token, no lane token and no Keycloak client reach the pod. The App's runtime secrets (`secretRef`) never reach a build.
- **Caps:** the Portal enforces bundle 64 MiB, image 128 MiB, component 64 MiB and log 1 MiB on the body stream, before any write. Today's checks stay: the commit is the head, and the digest and sbom come from the same build.
- **The wasm host:** the same binary and limits (64 MiB, 5 s, fuel 2e11, 2/8 MiB bodies, 60 s jobs), per-shard keys and one schema per App.
- **Limits are per deployment.** No limit is keyed by organization. This deliberately departs from ekiben's `OrgQuotas` (`crates/ekiben/crates/ekiben-core/src/org.rs:46-98`).

### 3.2 Threats

| threat | how it would go | control |
|---|---|---|
| malicious App source | `build.rs`, a proc macro, a vite plugin or a test runs attacker code during the build | The code runs only in the build pod, under the sandbox in 3.1. The one secret it can read (from `/proc`, as in dev) is the build token. That token can only fetch the source of this commit and upload this build's artifact. The App's author already controls that artifact by definition. Every build starts fresh: the pod exits after one build. The cache PVC is per App, so a poisoned cache reaches only the same App. The bundle is served on the App's own origin with SRI and a per-App CSP. |
| dependency confusion | the App's `package.json` or `Cargo.toml` names a package an attacker controls on a public registry | No install step exists. `node_modules` come from the image template, and cargo runs `--offline --locked` over the image's crate store. Egress has no registry, so a fetch fails closed. Test: a build naming a crate absent from the store fails with an offline error. |
| egress exfiltration | build code sends source, tokens or data out | The NetworkPolicy allows jc's port and DNS only. The pod holds no data credential; the build token cannot read data. Residual: DNS queries leave the pod as in dev. A test resolves an outside name and connects, and must fail at connect. |
| cross-project artifact read | App X's build fetches App Y's source or overwrites Y's package | The build token is bound to one build id. The source route resolves the repository from the build record, never from the request. Artifact puts are keyed by the build id, and the Portal publishes `app-{name}` with its own token. Tests: a token of build X on build Y's routes gets 404; an expired token gets 401; a token for a finished build gets 409. Packages are read by the Portal only (`fetch.rs`). The wasm host's per-shard (not per-App) store key is dev's existing model and stays unchanged. |
| resource exhaustion | a flood of pushes, a fork bomb, a huge bundle, a never-ending test | One pending build per App: a newer head replaces the queued one. `MAX_RUNNING = 1` per deployment, FIFO. The deadline, CPU and memory limits are as in 3.1, plus the new ephemeral-storage limit. Body caps are enforced while streaming. The log is capped at 1 MiB. `ttlSecondsAfterFinished 600` cleans up. A Rebuild is refused while a build of the same commit is pending. |
| a forged build result | a caller who is not the build pod sets `status.build` | Only the Portal writes `status.build`, from a build record whose artifact it hashed itself. The lane token and the `build-lane` Role are retired after d lands. Until then, the propose route keeps its current checks. |

## 4. Slices, smallest first

Every slice runs the charts-apps suite on the real dev export
(`bins/jc/smoke/run charts-apps --export /workspace/evidence/jc-rehearsal/export/dev …`). Before
that, fix the suite: `app.repo` needs the shako token in `--state`, which today skips all 80
repository checks, and a role holder must sign in for the 4 + 1 roles Apps.

### Slice 1: ship the 27 bundles in the jc image (option f)
- **Brief:**
  - Copy the `apps` stage of `crates/portal/Dockerfile:54-190` into `bins/jc/Dockerfile`, with the same `app-integrity.mjs` step, and `COPY --from=apps /srv/apps /srv/apps`. Prefer one shared stage file over a second copy.
  - Set `JC_PORTAL_APPS_DIR=/srv/apps` in jc's Portal sibling wiring (owner of `portal.rs`: jc-portal-integrate).
- **Done when:** `GET /apps/{name}/` answers 200 text/html for the 23 non-roles shipped Apps, and 403 for the 4 roles Apps without the role and 200 with it. `app.page` reads data for the public ones.
- **Tests:**
  - a jc test asserting `apps_dir` is set when `/srv/apps` exists;
  - `scripts/ci/check-dockerfile-members.py`, or an equivalent check that the two Dockerfiles list the same App set.
- **Security:** no new route. Integrity is checked per file as today.
- **Measures:** the image size before and after, and the image build time; `app.served` pass count (expected 27, with 4 as 403-by-design).

### Slice 2: carry dev's builds (option e)
- **Brief:**
  - jc-migrate's export reads, for every App with `status.build`, generic package `app-{name}` version `{commit}-{digest12}`, and image `app-{name}@{digest}` for the 2 ui-rust Apps. It writes them into the export.
  - The import publishes them to shako under the `jc-apps` owner.
  - jc sets `JC_PORTAL_APPS_CACHE_DIR` to a writable directory under its state.
- **Done when:** the 4 ui and 10 wasm Apps of class B/C serve their static pages from the fetched build. A tampered package (one byte changed) is refused by `fetch.rs` and reported `BuildMissing`.
- **Tests:**
  - an import test that runs the round trip, including the digest refusal;
  - a reconciler test that a cache dir plus a package gives Live.
- **Security:** the digest is checked at fetch. The export carries no token. The packages are written by the import's admin credential only.
- **Measures:** `app.served` (expected 27 + 14), the package bytes carried, and the fetch time per App on the first sync.

### Slice 3: wasm host beside jc
- **Brief:**
  - Run `jc-wasm-host` (the platform image) and apps-db beside jc, with an S3 store (RustFS in the rehearsal), as `CUTOVER.md` keeps them.
  - Wire `JC_PORTAL_ARTIFACT_STORE_*`, `JC_PORTAL_APPS_DB_URL` and `JC_PORTAL_WASM_*`.
  - First measure how the host reads placements without Kubernetes. Today the Portal writes ConfigMap `jc-wasm-host-<n>-placements` (`reconciler/wasm_shards.rs:100-150`). If the host reads a mounted file, the compose variant mounts a directory the Portal writes; if it reads the API, the slice reports that before adding anything.
- **Done when:** the 10 wasm Apps' `/api/*` reads answer 2xx in `app.page`, and one `spec.server.jobs` export runs under the 60 s cap.
- **Tests:**
  - a reproduction of the limits: a request over 5 s gets 504/503, and a body over 2 MiB gets 413;
  - one App cannot read another's blob prefix.
- **Security:** NetworkPolicy and compose network: the host reaches jc's gateway port, apps-db and the store only; ingress is from the edge and the Portal admin port.
- **Measures:** host RSS with 10 Apps, the first-request latency (cold component), and the p50 of the steady state.

### Slice 4: the fixed build lane, Kubernetes mode (option d)
- **Brief:**
  - **Trigger:** a reconciler pass compares each published forge App's default-branch head (one ref read from shako) with `status.build.commit` and the pending build. Rebuild sets a pending build.
  - **Dispatch:** `build_pods.rs` keeps its pod spec. The `runner` container runs `build-app` and then `lane.mjs put` instead of act_runner. The Secret holds the build token in place of the registration token.
  - **Routes:** `/internal/app-builds/{id}/{source.tar,artifact/*,log}`, served only behind jc's internal listener or a NetworkPolicy, with token auth in constant time.
  - **Completion:** the Portal runs `check_and_publish`'s checks over the stored artifact, publishes, and commits `status.build` as the Portal's own writer.
  - **Build history:** the Portal keeps its own build table (id, App, commit, state, times, log) in place of `actions/runs` (`api/app_build.rs:174,251,304`).
  - The workflow files in the repositories stay untouched and unread.
- **Done when:** on a jc in k3s (stage-like), a push to `jc-apps/helsinki_t6429-bike-live` main builds and publishes it, and `/apps/t6429-bike-live/` serves. A Rebuild of alerts-desk rebuilds the same digest (reproducible) or records the new one.
- **Tests:**
  - unit tests per route: no token gets 401; another build's token gets 404; an expired token gets 401; a finished build gets 409; an oversize body gets 413 before any write;
  - a digest mismatch is refused;
  - the commit is not the head: refused;
  - a red proof by reverting the token compare;
  - a pod-spec snapshot equal to dev's spec, except the command and the token.
- **Security:** threat table 3.2. Add the `ephemeral-storage` limit from this slice's measurement. Retire `JC_LANE_TOKEN` and the `build-lane` Role in the deployment only after the propose route is no longer used.
- **Measures:** build wall time and peak RSS per class (node, wasm, rust) on helsinki-events, kpi-forecast and air-quality; the `/tmp` peak bytes (which sets the storage limit); the source.tar size; whether each build is reproducible (digest equal to dev's committed digest).

### Slice 5: standalone builder (compose / rehearsal)
- **Brief:**
  - A `jc-builder` service on the builder image, non-root, read-only root, no docker.sock, on a compose network that reaches only jc.
  - It long-polls `POST /internal/app-builds/claim` with a builder credential read from a file. The Portal keeps one per deployment and rotates it per claim. It gets one build token, runs, and exits.
  - `restart: always` brings it back fresh.
- **Done when:** the rehearsal builds t6429-bike-live end to end with no Kubernetes.
- **Tests:** a claim with no or a wrong credential gets 401; two claims never get the same build; a builder that dies mid-build lets the build expire and requeue once.
- **Security:** the claim credential can only claim. It is useless without a pending build.
- **Measures:** the same as slice 4, so the two modes can be compared.

### Not in these slices
- **ui-rust serving in a standalone jc** (class D, 2 Apps). It needs a container runtime. On Kubernetes, slice 4 builds and pushes the image, and the existing reconciler runs it. Standalone stays a documented gap.
- **Arbitrary workflows (c2):** only if the owner asks for CI in App repositories.
