# ADR-0027 — Surfaces architecture: the vanilla binary, plus feature-gated surface crates in one binary

Date: 2026-10-10. Status: accepted. Decided by worker-2 under the owner
order of 2026-10-10 ("a design decision is yours: pick the recommended
option"); the owner may override any rule below. The master decision of
the surfaces epic (`tasks/GOAL-antares-surfaces.md`), built on ADR-0013,
ADR-0014, ADR-0016, ADR-0017, ADR-0020, ADR-0023 to ADR-0026 and the
analyses in `docs/analysis/` (T-3435 to T-3442, T-3447 to T-3449). Amends
ADR-0025 rule 8 and ADR-0026 rule 1 (section 9). Task T-3443.

## Context

The owner wants the platform's filtered endpoints inside the broker, as
optional modules of one Rust binary (GOAL points 1 to 3, 6). The analysis
round produced:

| input | what it fixed | file |
|---|---|---|
| T-3435 | TS 104 175 / 104 176 delta of today's broker, gap tasks T-3450 to T-3461 | `docs/analysis/ts104175-delta.md` |
| T-3436 | what moves from `context-gateway` and what stays (D1 to D13) | `docs/analysis/gateway-to-broker-map.md` |
| T-3437 | every TS 104 175 operation against six surfaces, with its ADR-0020 checks | `docs/analysis/operation-matrix.{md,yaml}` |
| T-3438 | endpoint views | ADR-0023 |
| T-3439 | internal context sources | ADR-0025 |
| T-3440 | WebSocket notification channel | ADR-0026 |
| T-3441 | OData and LinkML mapping | `docs/analysis/odata-linkml-mapping.md` |
| T-3442 | MCP surface | `docs/analysis/mcp-surface.md` |
| T-3447 | threat model, module rules M1 to M4, code rules, supply chain | `docs/analysis/surfaces-threat-model.md` |
| T-3448 | build our own `odata-*` crates | ADR-0024, `docs/analysis/odata-library.md` |
| T-3449 | an external conformance suite per surface | `docs/analysis/surfaces-conformance.md` |

The seams already exist: `ApiSurface` mounts a router below `/x`
(`crates/antares-api/src/surface.rs:17-50`, selected by
`ANTARES_API_SURFACES`, `crates/antares-broker/src/main.rs:455-486`);
`NotificationSink` is the whole binding contract (ADR-0016); `Engine` is
the policy seam (ADR-0020); `AppState::call` runs one request through the
broker's own router, "negotiation, the bounds wall, tenancy, the policy
seam, history and notifications ... exactly once"
(`crates/antares-api/src/state.rs:535-636`). What is missing is the
operation layer a surface may depend on without reaching the store
(threat model F1: today a surface could only take `AppState`, which hands
it the store), and the rules that keep a vanilla build vanilla.

Measured cost of the seam the surfaces will use (`docs/src/performance.md`,
façade section, release build, in process): a 100-Entity answer costs
66-71 µs more through `AppState::call` than the request it wraps, an
empty answer under 10 µs; "about 0.7 µs per Entity on this shape".

## Decision

### 1. Crates and cargo features

Adopted from the threat model section 9.1 without change:

| crate | role | `antares-broker` feature | depends on (workspace) |
|---|---|---|---|
| `antares-ops` | the operation layer: `Ops` (private fields, wraps `AppState::call`), `OperationId` (closed set of `operation-matrix.yaml` row ids), the view table and its compiler (section 5), the error mapping (threat model 10.3), the seam types re-exported as `antares_ops::policy` | pulled by any surface feature | `antares-api` (feature `views`), `antares-model`, `antares-ql` |
| `antares-views` | NGSI-LD view root `/x/views/{view}/ngsi-ld/v1` (ADR-0023) | `views` | `antares-ops` |
| `antares-odata` | OData 4.01 `/x/views/{view}/odata/v4` (ADR-0024, `odata-linkml-mapping.md`) | `odata` | `antares-ops`, the `odata-*` crates, `linkml-model` |
| `antares-mcp` | MCP `/x/views/{view}/mcp` (`mcp-surface.md`) | `mcp` | `antares-ops`; registry crate `rmcp = "=3.5.1"` |
| `antares-linkml` | LinkML schema `/x/views/{view}/linkml` | `linkml` | `antares-ops`, `linkml-model` |
| `antares-ws` | WebSocket notifications (ADR-0026) | `ws` | `antares-ops` |
| `antares-icsr` | internal context sources (ADR-0025) | `internal-csr` | `antares-ops` |
| `odata-url`, `odata-csdl`, `odata-json`, `odata-batch`, `odata-axum` | protocol libraries (ADR-0024 decision 1) | none of their own | each other only; never an `antares-*` crate |
| `linkml-model` | protocol library | none of its own | none in the workspace |
| `antares-residual` | role `ops`: the residual evaluator of ADR-0028 (rule 2), pure, re-exported by `antares-ops` | none of its own | none in the workspace |

`full = ["views", "odata", "mcp", "linkml", "ws", "internal-csr"]`;
`default` stays `["mqtt"]` (`crates/antares-broker/Cargo.toml:56`), so
the default build is vanilla. Every workspace package declares
`[package.metadata.antares] role = "core" | "ngsild" | "ops" | "surface" |
"protocol" | "addon"` (threat model 9.1; `ngsild` and its rule M3 added by
T-3910, `docs/analysis/ngsild-crates.md`).

Decided (worker-2, owner may override): the threat model's feature names
(`views`, `odata`, `mcp`, `linkml`, `ws`, `internal-csr`), not
`surface-*` prefixes, because T-3443 adopts the module rules verbatim and
those rules name these features in their CI checks; options: plain names
(chosen), `surface-` prefixes.

Decided (worker-4, owner may override, T-3513): the four view surfaces
share one prefix, `/x/views`, the view root of ADR-0023 rule 10; each
serves its own sub-root after `{view}` (`ngsi-ld/v1`, `odata/v4`, `mcp`,
`linkml`). A surface whose prefix is exactly `/x/views` is merged with
the others of that prefix under one nest, so `/x/views/{view}/mcp` sits
beside `/x/views/{view}/ngsi-ld/v1` as this table names them; two such
surfaces declaring the same route stop the broker at start. Every other
prefix keeps the refusal of an equal or nested one
(`crates/antares-api/src/surface.rs`, `overlaps`). Options: one shared
view root (chosen), a prefix per surface (`/x/mcp/{view}`, which breaks
the paths of this table and of ADR-0023 rule 10), one surface that
dispatches to the others (a surface depending on a surface, against
module rule M1).

No surface touches `antares-store` or `antares-sql`: every operation goes
through `Ops::run`, i.e. through the handler that serves the same
operation over NGSI-LD, with its `gate!` call and the ADR-0020 seam. The
sink contract (`NotificationSink`, `Outbound`), `ApiSurface` and
`ForwardTransport` reach the surfaces as `antares-ops` re-exports of
`antares-api` items, so no surface names `antares-api` or
`antares-notifier` (rule M2). `antares-ops` itself re-exports through
`antares-api`, which today re-exports `DeliveryPolicy` and `page_sink`
from `antares-notifier` (`crates/antares-api/src/lib.rs:77`, `:137`) but
not `NotificationSink` or `Outbound`; `antares-api` gains
`pub use antares_notifier::{NotificationSink, Outbound, DeliveryFuture, DeliveryError}`,
so `antares-ops` needs no edge to `antares-notifier`.

### 2. Module rules: adopted verbatim

Sections 9.3 to 9.6 of `docs/analysis/surfaces-threat-model.md` are
normative for this ADR as written there, including their enforcement
scripts, CI steps, red conditions and self-tests:

- **M1** each surface is its own crate behind its own cargo feature; a
  protocol library has no dependency on any `antares-*` crate
  (`dev/check-module-rules.py`, step "Module rules" in `workspace.yml`).
- **M2** a surface crate depends only on `antares-ops` and its protocol
  libraries; never on `antares-store` or SQL (same script, plus the
  symbol-level Cypher of 9.4 for reviewers and T-3446).
- **M3** feature off = no route registered and no code linked
  (`dev/check-no-addon.sh` extended, `dev/check-vanilla-symbols.sh`,
  `cargo auditable` package list of the image, the shelf test).
- **M4** feature on but no view (or, for `internal-csr`, no export grant)
  = the surface is closed, 404 and reported in `/q/health`; nothing ever
  serves allow-all.

A change to any of them is a new ADR.

### 3. Proof that vanilla is vanilla

Three independent proofs, all in CI, all red on a difference:

1. **Dependency tree.** `cargo tree -e features -p antares-broker` of the
   default build is committed as `ci/vanilla-tree.txt`; a step diffs it
   on every pull request. A diff is red unless the same commit updates
   the file, so a change to the vanilla tree is always a reviewed line.
   The step also requires that none of the M3 forbidden names occurs in
   it (`dev/check-no-addon.sh`).
2. **Symbols and package list.** `dev/check-vanilla-symbols.sh` on the
   unstripped vanilla debug binary and on the shipped image's embedded
   `cargo auditable` list (M3 items 2 and 3).
3. **ETSI result.** The per-TP verdicts of the vanilla build are the
   reference (`surfaces-conformance.md` run E1, the committed baseline of
   T-3466); the `full` binary on the plain root (E3) and through the view
   `all` (E2) must give the same verdict TP by TP.

### 4. Feature matrix in CI

- Every pull request: `cargo hack --each-feature --no-dev-deps check -p
  antares-broker` and `cargo hack --each-feature clippy --all-targets -p
  antares-broker -- -D warnings`, then `cargo test -p <surface crate>`
  for each surface crate. T-3465 builds the job for today's features; the
  surface features join it as each crate lands.
- Nightly: `cargo hack --feature-powerset --depth 2 check -p
  antares-broker --features mqtt` over the six surface features (pairs
  catch the interactions that exist: every surface with `views`, `ws`
  with `internal-csr` through distributed subscriptions), and `full`
  with and without `mqtt`.

Decided (worker-2, owner may override): `--depth 2` instead of the full
powerset, because the six features meet only through `antares-ops`,
which every one of them enables, so an interaction needs at most two
surfaces; the full powerset is 64 builds per night for no further edge.
Options: `--depth 2` (chosen), full powerset, `--each-feature` only.

### 5. Shared pieces: one query language, one view compiler

- **Query.** `antares-ql` already holds the one AST and both evaluators:
  the in-memory evaluator (`crates/antares-ql/src/eval.rs`) and the SQL
  compiler (`antares_ql::sql::compile_q`, called by `antares-sql`; cg:
  `MATCH (c:Symbol)-[:CALLS]->(f:Symbol) WHERE c.package='antares-sql' AND
  f.package='antares-ql' AND NOT c.is_test RETURN DISTINCT f.path`). Every
  surface translates its own filter language into this AST and never
  into SQL or a query string: OData `$filter` → `QNode`
  (`odata-linkml-mapping.md` section 5), MCP tool arguments → the NGSI-LD
  request (`mcp-surface.md`), the view predicate → `QNode` at load. T-3462
  holds that the two evaluators agree (property test); this ADR makes that
  test a required check for every surface.
- **Views.** One view compiler in `antares-ops`: load, validate (types
  expanded, `q` parsed once into `QNode`, operation ids checked against
  the matrix), and hand `selector ∧ q ∧ scope_q ∧ geo_q` plus the
  attribute allowlist to the `gate!` composition hook (ADR-0023 rule 1).
  The NGSI-LD view, OData, MCP, LinkML and WebSocket surfaces all serve
  through it; none has a view model of its own.
- **Errors.** One mapping in `antares-ops`, rendered per surface (threat
  model 10.3), so "hidden" and "absent" are the same bytes on every
  surface.

### 6. Performance budget

Measured against the vanilla numbers T-3467 records, same machine, same
commit, same shape (`dev/perf/shapes.sh` on the `perf-weekly.yml`
instance type):

| surface | budget against the same query on plain NGSI-LD | why this number |
|---|---|---|
| NGSI-LD view | p50 ≤ 1.05×, p99 ≤ 1.10× | a view is a conjunct on the AST and an allowlist on the projection; it runs in the same handler, no extra serialisation |
| OData read | p50 ≤ vanilla + 0.15 ms + 1 µs per Entity, p99 ≤ 1.25× | one `AppState::call` (66-71 µs per 100 Entities measured) plus the JSON Format rewrite |
| MCP tool call | p50 ≤ vanilla + 0.2 ms + 1 µs per Entity, p99 ≤ 1.25× | one `AppState::call` plus JSON-RPC framing |
| LinkML document | ≤ 5 ms p99 for a 100-class schema, cached per view revision | computed once per view revision |
| WebSocket notification | delivery latency p99 ≤ 1.10× HTTP sink p99 to a local receiver (T-3626); ≤ 32 KiB resident per idle connection plus its queue | no socket opened per notification; one queue hop per connection |
| internal CSR forward | p99 ≤ 0.5× the HTTP forward to the same broker | no socket, no TLS, no second JSON parse of the request line |
| binary size | `full` release binary ≤ vanilla + 15 MiB; each feature alone ≤ vanilla + 15 MiB (owner 2026-10-22, T-5246; 10 MiB from 2026-10-11, T-3775) | measured with `ls -l` of the stripped release binary per feature in the feature-matrix job |
| vanilla | unchanged within the noise band of T-3467 | GOAL point 1 |

A benchmark per row: criterion benches in each surface crate for the
in-process cost (translation, rendering) and `dev/perf/shapes.sh` shapes
`view`, `odata`, `mcp` for the end-to-end numbers. A regression past a
budget fails the nightly job once T-3467's noise profile exists (its
"ten runs" rule, `docs/src/performance.md`); until then the job reports.

Decided (worker-2, owner may override): relative budgets with a fixed
term for the translating surfaces, because the façade measurement shows
a near-zero fixed cost and a per-Entity cost, so a pure ratio would be
too loose on large pages and too tight on empty ones. Options: ratios
only, absolute milliseconds, ratio plus fixed term (chosen).

The `mcp` budget, measured 2026-10-11 on dev 10c2946
(T-3662), stripped thin-LTO release binary, x86_64, default features plus
the one named: vanilla 31 033 392 bytes; `views` 32 279 216 (+1.19 MiB);
`mcp` 35 405 648 (+4.17 MiB). `mcp` implies `views`, so its own share is
2.98 MiB. `cargo bloat --crates` puts 0.78 MiB of `.text` in
`antares_mcp` (32 tools, their argument checks and schemas), 0.68 MiB in
`rmcp` (the protocol, Streamable HTTP, request state) and 0.5 MiB more in
`std` (their generic instantiations); no single function is above
40 KiB. `opt-level = "s"` on `antares-mcp`, `rmcp` and `schemars` gives
34 607 792 (+3.41 MiB): still over 2 MiB, and the shared host's
`benches/call.rs` noise (the unchanged root query moved from 107 to
170 µs between runs) cannot show it costs a tool call nothing, so it is
not taken.

Decided (worker-2, owner may override): `mcp` ≤ vanilla + 4.5 MiB,
because 2 MiB is reachable only by dropping rmcp for a hand-written
JSON-RPC and Streamable HTTP layer or by dropping tools, and the first
trades bytes for a protocol implementation nobody else tests while the
second removes MCP behaviour. Options: raise the `mcp` budget (chosen),
measure each feature over the feature it implies (`mcp` over `views`,
2.98 MiB, still over), size-optimise the MCP crates (+3.41 MiB, still
over), replace rmcp.

The `odata` budget, measured 2026-10-11 on dev 8c42bcd (T-3705), same
build: vanilla 31 106 992 bytes; `views` 32 374 448 (+1.21 MiB);
`odata` 34 439 120 (+3.18 MiB). `odata` implies `views`, so its own
share is 1.97 MiB, inside the per-feature 2 MiB. `cargo bloat --crates`
against the `views` build puts 348 KiB of `.text` in `antares_odata`,
135 KiB in `linkml_model` (the Tenant's schema that `$metadata` reads),
115 KiB in `odata_url`, 279 KiB more in `serde_json` and 307 KiB more in
`std` (generic instantiations). The `serde_json` share is its
`float_roundtrip` feature, which ADR-0024 requires of `odata-json` and
`odata-batch` (T-3504, T-3505) so an `Edm.Double` reads back to the same
bits; Cargo unifies it for the whole binary.

Decided (worker-4, owner may override): `odata` ≤ vanilla + 3.5 MiB,
because its own share over `views` meets the 2 MiB rule and the rest is
`views` plus a float parser that ADR-0024 makes a correctness property.
Options: raise the `odata` budget (chosen), drop `float_roundtrip` (an
`Edm.Double` no longer round-trips, ADR-0024 broken), measure each
feature over the feature it implies (changes the rule for every feature),
size-optimise the OData crates (saves part of 0.6 MiB, still over 2 MiB).

The `ogc` budget, measured 2026-10-11 by `dev/preflight` on the T-3745
branch (stacked on T-3759), same build: vanilla 31 086 672 bytes;
`views` 32 391 184 (+1.24 MiB); `ogc` 33 568 368 (+2.37 MiB). `ogc`
implies `views`, so its own share is 1.13 MiB, inside the per-feature
2 MiB. Part 3 (T-3745) brings the CQL2 text and JSON parser
(`ogc-cql2`) and the residual evaluator (`antares-residual`, shared
with `odata` in `full`) into the `ogc` build.

Decided (worker-6, owner may override): `ogc` ≤ vanilla + 2.5 MiB,
because its own share over `views` meets the 2 MiB rule and the rest
is `views` plus the residual that OGC Part 3 needs for what `q` cannot
say. Options: raise the `ogc` budget (chosen), push only what `q`
says and answer 501 for the rest (drops most CQL2 conformance classes),
measure each feature over the feature it implies (changes the rule for
every feature).

The `full` budget, measured 2026-10-11 in the `binary-size` job of `fast`
(stripped thin-LTO release binary, x86_64): on dev 7b22710 (run
38143777118) vanilla 31 110 424 bytes and `full` (views, mcp, odata, ws)
37 352 760 (+5.95 MiB, 0.05 MiB under the old 6 MiB); with `ogc` on
train 9f4270d (run 38143787514) `full` 37 690 456 (+6.28 MiB) and `ogc`
alone 32 943 928 (+1.75 MiB, inside its 2 MiB). `ogc` adds 0.32 MiB to
`full`, because `views`, `serde_json` and axum are shared. The 6 MiB was
set for four surfaces; `full` is the union of every surface, so it grows
with each one even when each stays inside its own budget.

Decided (worker-4, owner may override): `full` ≤ vanilla + 7 MiB,
because the per-feature budgets still bound each surface and the union of
five sits at +6.28 MiB, leaving 0.72 MiB for the CQL2 filter (T-3745)
before this row is read again. Options: raise `full` (chosen), shrink
`antares-ogc` by 0.28 MiB (its own share over `views` is 0.53 MiB, so
this would cut most of the surface), `opt-level = "s"` on the surface
crates (rejected for `mcp` in T-3662 for the same unmeasurable call
cost), fat LTO for the release profile (slower release builds for every
lane, not measured here).

The WebSocket notification budget, measured 2026-10-11 on dev f3db73d
(T-3626, `bench-ws.yml` run 38127455609 on ubuntu-latest), five runs of
`cargo bench -p antares-ws --bench fanout`, 2000 sequential changes, each
channel on its own Car and Subscription:

| run | socket p50 | socket p99 | HTTP p50 | HTTP p99 | p99 ratio |
|---|---|---|---|---|---|
| 1 | 2.018 ms | 4.017 ms | 1.899 ms | 3.933 ms | 1.021 |
| 2 | 2.000 ms | 4.306 ms | 1.895 ms | 4.235 ms | 1.017 |
| 3 | 1.980 ms | 4.065 ms | 1.851 ms | 4.093 ms | 0.993 |
| 4 | 1.985 ms | 4.054 ms | 1.822 ms | 3.914 ms | 1.036 |
| 5 | 1.990 ms | 3.988 ms | 1.795 ms | 3.822 ms | 1.044 |

The socket pays one hop the HTTP sink does not: the sink hands the
notification to `Conn::offer`, which queues it, and the connection's
session task writes it; the HTTP sink writes from the notifier task.
That queue is the connection's bounded backlog, its coalescing and its
resume from a seq, and it keeps a slow client from holding the notifier.

Decided (worker-1, owner may override): socket p99 ≤ 1.10× HTTP sink
p99, because the measured gap (0.99× to 1.04×, 0.12 to 0.2 ms at p50) is
that one hop, and 1.10× is the noise margin the NGSI-LD view row already
uses. Options: raise the bound (chosen), write to the socket from the
notifier task (one slow client stalls every Subscription's delivery),
keep the 1.0× bound (fails on noise alone, run 3 is below, run 5 above).

Decided (owner, 2026-10-11, T-3775): one ceiling, `full` and each feature
alone ≤ vanilla + 10 MiB. The per-feature 2 MiB rule and its exceptions
(`mcp` 4.5, `odata` 3.5, `full` 6 then 7 MiB) cost a budget fight on every
new surface and two red trains on 2026-10-11 for growth the owner accepts.
The gate stays, so a surface that pulls in tens of MiB is still red.

Decided (worker-t5234, owner may override; T-5234, 2026-10-22): fat LTO
for the release profile, because dev 827c303 put `full` 3040 bytes under
the 10 MiB ceiling and nothing short of it frees 512 KiB without a CPU
cost. Measured on one host, stripped release binaries: thin LTO vanilla
32 333 224, `full` 42 815 880 (+10.00 MiB); one codegen unit for the cold
crates and `antares-ogc` brings `full` to 42 389 384 (+9.59 MiB); fat LTO
on top gives vanilla 27 042 344 and `full` 35 820 040 (+8.37 MiB). Broker
CPU per request (`/proc` utime+stime over a 10 s k6 run at c20, three
rounds, `dev/perf/shapes.sh` setup) falls on every shape: `query` 267 to
251 µs, `view` 346 to 334, `odata` 480 to 444, `mcp` 680 to 653, `ogc`
493 to 450. A release link takes about 3 min longer (53 s to 231 s for
`full`), so the `binary-size` job runs on six runners instead of three.
Options: fat LTO (chosen); `opt-level = "s"` on `antares-ogc` (-227 KiB,
but 7 % more CPU per OGC request, 488-492 to 520-531 µs); `opt-level =
"s"` on `rmcp` (grows the binary by 10 KB); dropping a feature from `full`
or raising the ceiling (both refused by the task).

Decided (owner, 2026-10-22 17:15Z, T-5246): the ceiling rises to vanilla
+ 15 MiB for `full` and for each feature alone, because the 10 MiB one
held every Antares train for about 3 hours on T-5234. The 2026-10-11
ceiling of 10 MiB (T-3775) stays in the history above; the gate still
turns red on a surface that pulls in tens of MiB.

### 7. Code rules for every new crate

Adopted from the threat model section 10.1 without change: each new
crate carries its own complete `[lints]` table (`unsafe_code = "forbid"`,
`missing_docs = "deny"`, clippy `pedantic`, `unwrap_used`, `expect_used`,
`panic`, `indexing_slicing`, `arithmetic_side_effects`, `todo`,
`unimplemented`, `dbg_macro`, `print_stdout`, `print_stderr` denied,
`broken_intra_doc_links` denied), `#![forbid(unsafe_code)]` in `lib.rs`,
the crate-local `clippy.toml` for test code, `overflow-checks = true` in
release per crate, no `allow(...)`. CI: `cargo clippy --all-targets
--all-features -- -D warnings`, `RUSTDOCFLAGS=-D warnings cargo doc`,
`cargo deny` (licenses, advisories, bans, sources), `cargo audit`,
`cargo machete`, Miri on `odata-url`, `odata-csdl`, `odata-json`,
`odata-batch`, `linkml-model` (section 10.1), parser limits passed by the
caller and a fuzz target per parser (10.2), the supply-chain gates of
section 11.1. The one unavoidable `unsafe` in a dependency the surfaces
add is named there: `tungstenite` 0.29.0 (five lines, threat model
11.2). OData: build our own crates, ADR-0024.

### 8. Rollout order, and what `context-gateway` keeps

Build order (each step its own batch, filed by T-3445; a later step
depends on the earlier ones it names):

1. Vanilla fixes that the surfaces stand on: T-3470 (join targets under a
   narrowing), the security fixes and TS 104 175 gaps T-3450 to T-3461,
   the baseline group T-3462 to T-3468. These change vanilla, so each
   keeps the ETSI result or improves it.
2. CI gates first: module rules M1 to M4, vanilla tree, symbol gate,
   supply chain, feature matrix (sections 2 to 4, 7). They are green on
   an empty set of surfaces and stay green as crates arrive.
3. `antares-ops` and the `views` feature in `antares-api` (the composition
   hook, `FILTERABLE` discovery, per-member batch, pre-image `Operation`:
   ADR-0023 consequences).
4. `antares-views` (NGSI-LD view), then the operation-matrix test
   (T-3444 plan item 2) against it.
5. Protocol crates `odata-url`, `odata-csdl`, `odata-json`, `odata-batch`,
   `odata-axum`, `linkml-model` (parallel batches; no broker dependency).
6. `antares-odata` read, then write with linked-entity checks;
   `antares-linkml`; `antares-mcp`; `antares-ws`; `antares-icsr` (any
   order after 4; each one batch).
7. Conformance jobs per surface (`surfaces-conformance.md` section 7) as
   each surface lands.
8. Per surface, in the platform: switch the callers to the broker surface
   (a test per caller), then delete the `context-gateway` code
   (GOAL point 11, inventory `gateway-to-broker-map.md` section 7).

`context-gateway` keeps (gateway-to-broker-map 4.2): D1 DCAT records and
catalogue feeds; D6 MCP hub and named servers, now forwarding each tool
call to the broker's MCP surface; D7 RFC 9728 protected-resource
metadata; D10 files, dumps, OGC Features and SensorThings as translators
re-based on the broker's view; D12 probes, metrics, telemetry; D13
authentication, token validation, audience, tenancy headers, rate limits
and quotas. Everything else in 4.2 moves (D2 to D5, D8, D9, D11).

### 9. Amendments to ADR-0025 and ADR-0026

- **ADR-0025 rule 8.** `ForwardTransport` is defined in `antares-api` and
  re-exported by `antares-ops`; `antares-icsr` implements the re-export
  and depends on `antares-ops` only (rule M2). Its `forward` takes the
  built request and the `Ops` the transport was constructed with, not
  `&AppState`: the signature becomes
  `fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>>`,
  and `Ops` gains the crate-private call `antares-icsr` needs through a
  method `Ops::forward_in_process(BuiltForward)` that passes the
  subject-only caller map ADR-0025 rule 2 requires.
- **ADR-0026 rule 1.** The cargo feature is `ws`, not `surface-ws`
  (section 1 of this ADR), and `antares-ws` reaches `AppState::call`
  through `Ops::run` and the sink contract through the `antares-ops`
  re-export.

## Consequences

- One binary, one image: `full` serves every surface; the default build
  serves what the broker serves today and proves it three ways.
- A surface author writes against `antares-ops` alone; the store is not
  reachable from a surface crate, by dependency and by symbol.
- The analysis-first hold on surface code ends when T-3443 (this ADR)
  and T-3444 (the test plan) are accepted; T-3445 files the round-1
  implementation tasks, each citing a section here.
- Every `AppState::call` costs a serialize and a parse (about 0.7 µs per
  Entity). The budget absorbs it; a typed operation layer stays closed
  until a surface measures it as its ceiling (`docs/src/performance.md`).

## Confirmation

| decision | check |
|---|---|
| 1, 2 | `dev/check-module-rules.py` (M1, M2) with `--self-test`; the M2 Cypher returns 0 rows in the T-3446 gate |
| 3 | `ci/vanilla-tree.txt` diff step; `dev/check-vanilla-symbols.sh`; ETSI E1 = E2 = E3 per TP |
| 4 | the `features` job of `workspace.yml` (each feature) and the nightly `--feature-powerset --depth 2` job |
| 5 | T-3462 property test required; `cg`: no surface symbol calls `antares_ql::sql::*` directly |
| 6 | criterion benches per surface crate; `shapes.sh` shapes `view`, `odata`, `mcp`; binary size per feature in the matrix job |
| 7 | each new crate's `[lints]` table present (module-rules script checks it); clippy, doc, deny, audit, machete, Miri steps |
| 8 | T-3445 files the batches in this order with these dependencies; T-3446 depends on all of them |
