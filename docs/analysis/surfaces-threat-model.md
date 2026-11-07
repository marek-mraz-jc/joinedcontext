# Surfaces threat model, module rules, code rules and supply chain (T-3447)

Sources: Antares-NGSI-LD-Context-Broker `62a566b` (branch `agent/worker-2/T-3436`; `git diff --stat 7185e1a..62a566b -- crates Cargo.toml dev .github` is empty, the commits since touch `docs/` only, so every code line cited here is also the code of `7185e1a`, the commit the `cg` graph was built at, `cg schema`: "built 2026-10-10T11:32:43Z @ 7185e1a100"). joinedcontext-platform was not read directly; its rows are cited through `docs/analysis/gateway-to-broker-map.md` (T-3436, platform `59a8f70`) and `docs/analysis/mcp-surface.md` (T-3442, read at 463 lines at `62a566b`). ETSI TS 104 175 V1.1.1 and TS 104 176 V1.1.1 under `/workspace/etsi-specs/`. crates.io API and `static.crates.io` tarballs read on 2026-10-10 (grep only, nothing built). Task T-3447; binds `tasks/GOAL-antares-surfaces.md` points 5, 6 and 8.

Inputs: `docs/analysis/operation-matrix.md` + `.yaml` (T-3437, "the matrix"), `docs/adr/ADR-0023-endpoint-views.md` (T-3438, its rules are cited as "ADR-0023 rule N"), `docs/analysis/gateway-to-broker-map.md` (T-3436, its security rows are cited as "map S1" to "map S26"), `docs/adr/ADR-0014-extension-hooks-model.md`, `docs/adr/ADR-0020-policy-seam.md`, `docs/analysis/odata-library.md` (T-3448 survey). `docs/analysis/odata-linkml-mapping.md` (T-3441, cited as "T-3441" with its section or row id: its `L1` to `L13` are LinkML path rows, its `W`/`Q`/`F` ids are OData rows, its `S1` to `S5` security items) and `docs/adr/ADR-0024-odata-library.md` (T-3448) appeared at `62a566b` while this was written and were read; the OData and LinkML rows below follow them. The LinkML threat rows of this page are numbered `LK1` to `LK8` so they cannot be confused with T-3441's path rows.

## 1. Scope, method and how to read the tables

Six surfaces, one section each (3 to 8): the NGSI-LD view, OData, MCP, LinkML, WebSocket notifications and internal (in-process) Context Source registrations. Section 2 holds what every surface shares: the trust boundaries, the assets, the bounds that already exist, the findings in today's code that every surface inherits, and the cross-surface STRIDE rows. Section 9 is the `module-rules` text T-3443 adopts verbatim. Sections 10 and 11 are the code rules and the supply chain.

STRIDE tables have five columns: `id`, `S/T/R/I/D/E` (Spoofing, Tampering, Repudiation, Information disclosure, Denial of service, Elevation of privilege; several letters when one threat is several), `threat`, `mitigation`, `test`. Test naming:

- An existing test is cited as `file::fn` with the file relative to `crates/antares-api/tests/` unless a path is given; every one was checked with `sed -n` on 2026-10-10 to be a `#[tokio::test]` or `#[test]`.
- A proposed test is marked "(proposed)". ADR-0023 already names the `views_*` tests of its rules (its Confirmation table); those names are reused unchanged and marked "(proposed, ADR-0023)". Proposed files follow the pattern of the existing ones: `crates/antares-api/tests/views_*.rs` (ADR-0023 puts them there), `crates/antares-odata/tests/odata_*.rs`, `crates/odata-*/tests/*.rs` for the protocol libraries, `crates/antares-mcp/tests/mcp_*.rs`, `crates/antares-linkml/tests/linkml_*.rs`, `crates/antares-ws/tests/ws_*.rs`, `crates/antares-icsr/tests/icsr_*.rs`, `crates/antares-ops/tests/ops_*.rs`. A proposed name of this page names the file it goes into, in the form `file.rs::fn`. Names taken from T-3441 (`docs/analysis/odata-linkml-mapping.md` section 10) and T-3442 (`docs/analysis/mcp-surface.md` section 10) are marked "(proposed, T-3441)" or "(proposed, T-3442)"; where the source gave no file, T-3444 assigns one.
- A row whose mitigation is a CI script names the script and the workflow step, and the script's `--self-test` is its test.

GOAL point 5 cases and where they are answered: inference through `q` on hidden attributes (V1, V2, V4, V9, O1), count and paging leaks (V3, O3, O4), out-of-view joins (V8, V9, O5), notification timing leaks (V14, W5). The extra cases of the task: request smuggling through `$batch` (O8 to O12), resource exhaustion by body size (G4, O11, MC6, W3), nesting depth (G4, O7, O11), `$expand` fan-out (O6), WebSocket connections (W2), MCP output size (MC5); cache poisoning of `$metadata` across views (O16, and LK2, MC9 for the same class on the other surfaces); confused deputy through internal CSR (C1 to C3).

## 2. What every surface shares

### 2.1 Trust boundaries

| # | boundary | who is on each side | what crosses it |
|---|---|---|---|
| B0 | gateway to broker | the platform gateway (authenticates, rate limits) and the broker | the request, the subject headers named by `ANTARES_POLICY_SUBJECT_HEADERS` (ADR-0020: "opaque to the broker — it parses no credential and validates no token") |
| B1 | surface crate to operation layer | the surface's protocol code and `antares-ops` (section 9.1) | one TS 104 175 operation per call, the caller's headers |
| B2 | operation layer to store | the NGSI-LD handlers behind `gate!` (`crates/antares-api/src/lib.rs:27`) and the store drivers | queries already narrowed by the seam |
| B3 | broker to egress | notification delivery, federation forwards | notifications after `pre_notify`, forwards stripped of subject headers (ADR-0020 "The subject never travels") |
| B4 | tenant to tenant | two tenants of one broker | only through a Context Source registration (ADR-0011) or, in section 8, an internal one |
| B5 | admin to broker | `/q`, which writes the view table (ADR-0023 rule 10) | configuration; outside the seam by ADR-0020 ("`/q` is outside the seam: the admin surface is the gateway's to protect") |

### 2.2 Assets

A1 hidden Entities (out of a view); A2 hidden Attribute values; A3 hidden Attribute and type names; A4 counts and paging state; A5 the tenant name (map S22); A6 the subject identity (ADR-0020); A7 the view configuration and the existence of other views; A8 broker availability (memory, connections, delivery slots); A9 integrity of stored Entities; A10 the conformance claim of the vanilla binary (GOAL point 1).

### 2.3 Bounds that exist today and apply to every surface

Every registered surface sits behind the same bounds wall as the NGSI-LD root: `crates/antares-api/src/lib.rs:487-498` nests each `ApiSurface` and layers `DefaultBodyLimit::max(*bounds::MAX_BODY_BYTES)` and `bounds::bounds_layer` over all of them ("a deployment's own /x routes are no less a way in"), proven by `api_surface.rs::a_registered_surface_is_behind_the_bounds_wall`. An inner `AppState::call` request passes the wall again (`facade_seam_f0.rs::an_inner_request_is_counted_by_the_bounds_wall`).

| constant | `crates/antares-api/src/bounds.rs` | value (source) | answer | used by rows |
|---|---|---|---|---|
| `MAX_BODY_BYTES` | 21 | 4 MiB default, `ANTARES_MAX_BODY_BYTES` (:21-27) | bare 413 | G4, O11, MC6, W3 |
| `MAX_URI_BYTES` | 28 | 8 KiB | bare 414 | O7 |
| `MAX_JSON_DEPTH` | 29 | 64 | 400 BadRequestData | G4, O11, MC6, W3 |
| `MAX_GEO_VERTICES` | 37 | 1024 (`crates/antares-ql/src/geo.rs:29`) | 400 | V23, O7 |
| `MAX_BATCH_ITEMS` | 42 | 1000 default, `ANTARES_MAX_BATCH_ITEMS` | 400 | O11 |
| `MAX_FED_RESPONSE_BYTES` | 54 | 16 MiB default | peer part skipped, warning | C5 |
| `MAX_FED_INFLIGHT` / `FED_INFLIGHT` | 65 / 72 | 256 default | callers wait | C5 |
| `MAX_FED_FANOUT` | 77 | 8 default | concurrency per request | C5 |
| `MAX_PEER_WARNINGS` | 92 | 8 | warnings truncated | C10 |
| `MAX_IN_PROCESS_CALL_DEPTH` | 107 | 8 | 500 InternalError | G2, C4 |
| `MAX_JOIN_LEVEL` | 109 | 10 | 400 | V8, O6 |
| `MAX_CONTEXT_FETCHES` | 114 | 32 (`crates/antares-jsonld/src/loader.rs:614`) | 400 | V23 |
| `MAX_Q_LINK_LOOKUPS` | 116 | 512 (`crates/antares-ql/src/eval.rs:17`) | budget per `q` | V9, O6 |
| `MAX_REGEX_CACHE`, `_BYTES`, `MAX_REGEX_PROGRAM_BYTES` | 119 | 1024, 64 MiB, 256 KiB (`crates/antares-ql/src/regex.rs:50-58`) | 400 | V23 |
| `MAX_Q_NODES` | 122 | 512 (`crates/antares-ql/src/lib.rs:266`) | 403 TooComplexQuery | V23, O7 |
| `MAX_FOLD_DOCS` | 132 | 100 000 default, `ANTARES_DISCOVERY_SCAN_MAX` | 403 TooManyResults | V7, O21, LK4 |
| `CHANGE_QUEUE` | 151 | 1024 | batch dropped and counted | W4 |
| `DELIVERY_WIDTH` / `_PER_TENANT` | 160 / 180 | 64 / 8 defaults | queued | W4 |
| `MAX_TRACKED_DESTINATIONS` | 204 | 4096 | LRU drop | W4 |
| policy `TIMEOUT` | `crates/antares-api/src/policy.rs:40` | 250 ms default, `ANTARES_POLICY_TIMEOUT_MS` | Deny | G3 |

Note on clause numbers: `bounds.rs` comments cite CIM 009 "6.3.4" for 411, 413 and 414. In TS 104 176 V1.1.1 the 413 and 411 rules are in `ts-104176-ngsi-ld-http-binding/6/6.3.2.md`: "'Request Entity too large' (413) which shall be raised when the HTTP input data stream provided by a client was too large i.e. too many bytes" and "'Length required' (411) ...". TS 104 176 6.3.4 is now "Expiration time for cached @contexts". The comment drift is recorded for T-3461 (re-audit of changed clauses); no 414 rule was found in TS 104 176 clause 6 (grep for `414`: no hit).

Every new cap a surface adds (proposed names in sections 3 to 8) is a `bounds.rs` constant or `LazyLock` knob, published in `LimitStats::snapshot` (`bounds.rs:215-243`), so the existing `bounds.rs::tests::every_declared_cap_is_published` (`bounds.rs:612`) and `the_documented_health_limits_are_the_published_ones` (`bounds.rs:423`) cover it. Values are chosen by the implementing task and the T-3443 ADR, not here.

### 2.4 Findings in today's code that every surface inherits

| # | finding | evidence | consequence | answered by |
|---|---|---|---|---|
| F1 | `AppState` exposes the store: `pub store: Arc<dyn CurrentStateDriver>` and `pub temporal` (`crates/antares-api/src/state.rs:78`, `:81`), and `ApiSurface::router(&self, st: AppState)` hands a surface the whole state (`crates/antares-api/src/surface.rs:28`). The reference surface already reads the store past the seam: `antares_plugin_example::surface::count` references `antares_api::state::AppState::store` at line 56 and calls `CurrentStateDriver::list` (`examples/plugin-example/src/surface.rs:51-60`) | cg, Cypher below | a surface built the same way answers without `gate!`, without a view and without `pre_notify` | G1, module rule M2 |
| F2 | `AppState::call` copies `NGSILD-Tenant`, `NGSILD-Snapshot`, `Link` and every subject header from the caller only "when it does not set them itself" (`state.rs:604-622`: `if parts.headers.contains_key(name) { continue; }`) | code | a façade or an internal forward that writes its own tenant or subject header is judged under that value: the confused-deputy path of section 8 | G2, C3 |
| F3 | the JSON depth scan runs only when `Content-Type` is absent or contains `json` (`bounds.rs:344-356`); a `multipart/mixed` OData `$batch` passes the wall unscanned | code | per-part JSON bodies need their own depth check | O11 |
| F4 | `echo_tenant_layer` echoes `NGSILD-Tenant` only when the caller sent it (`lib.rs:693-711`); an inner `AppState::call` response carries the tenant the façade set | code | a view or façade response can name the pinned tenant to a caller who never sent it (map S22) | V18 |
| F5 | fixed by T-3475: the per-commit `cargo deny` in `.github/workflows/workspace.yml` runs `check all` (licenses, advisories, bans, sources) and `deny.toml` bans `openssl-sys`, `native-tls`, `aws-lc-sys`; before it ran `check licenses advisories` only and `bans`/`sources` waited for the weekly `advisories.yml` | workflow | a new git or unknown-registry source, or a banned crate, is red per commit | section 11 |
| F6 | `[profile.release] strip = "symbols"` (`Cargo.toml:144`), and `full.yml` strips the release binary again | Cargo.toml, `full.yml` "Build + package" | a symbol check cannot read the release binary; it needs an unstripped build | module rule M3 |
| F7 | clippy runs `cargo clippy --locked --workspace --all-targets -- -D warnings`, without `--all-features` (`workspace.yml:150`) | workflow | code under `#[cfg(feature = "...")]` in `antares-broker` and `antares-api` is not linted; GOAL point 8 asks for `--all-features` | section 10 |
| F8 | tungstenite 0.29.0 (the version `antares-ws` takes through `tokio-tungstenite 0.29`) defaults to `max_write_buffer_size: usize::MAX`, `max_message_size: Some(64 << 20)`, `max_frame_size: Some(16 << 20)` (`tungstenite-0.29.0/src/protocol/mod.rs:100-102`) | crate tarball | an unbounded write buffer per slow WebSocket client and a 64 MiB message above the 4 MiB body cap | W3, W4 |
| F9 | the workspace clippy table sets only `unwrap_used = "warn"` and allows `needless_borrows_for_generic_args` (`Cargo.toml:162-166`); every crate inherits it with `[lints] workspace = true` | Cargo.toml | the GOAL point 8 denies are not in force anywhere | section 10 |

```cypher
// F1: who outside antares-api touches AppState::store (7 rows; the plugin surface is one)
MATCH (s:Symbol)-[r:REFERENCES]->(f:Symbol)
WHERE f.kind='Field' AND f.name='store' AND f.path CONTAINS 'AppState' AND s.package <> 'antares-api'
RETURN s.package, s.path, r.line;
// F1: the plugin crate's direct store calls outside tests
MATCH (s:Symbol)-[r:CALLS|REFERENCES]->(t:Symbol)
WHERE s.package='antares-plugin-example' AND t.package IN ['antares-store','antares-sql'] AND NOT s.is_test
RETURN s.path, t.path;
```

### 2.5 Cross-surface STRIDE rows

| id | S/T/R/I/D/E | threat | mitigation | test |
|---|---|---|---|---|
| G1 | E, I | a surface reads or writes the store directly (F1) and skips `gate!`, the view and `pre_notify` | surfaces get an `Ops` handle from `antares-ops`, never `AppState`; `Ops` has no store field and no `pub` accessor to one; module rule M2 makes a store edge red in CI | `dev/check-module-rules.py --self-test` (proposed, section 9); `crates/antares-ops/src/lib.rs` doc test `Ops` with a `compile_fail` block that names `ops.store` (proposed) |
| G2 | S, E | a façade sets its own `NGSILD-Tenant` or subject header on the inner request (F2) and is judged under it | `antares-ops` builds every inner request itself: its request type has no field for the tenant, the snapshot or a subject header; it copies them from the outer request and overwrites any value a surface put into a free-form header list | `crates/antares-ops/tests/ops_run.rs::a_surface_cannot_set_tenant_snapshot_or_subject`; `facade_seam_f0.rs::the_handle_carries_the_headers_that_decide_what_is_served` |
| G3 | E | an operation reached through a surface is not gated, or gated twice | every surface operation is one `AppState::call` into a route that already calls `gate!` once; the route walk is extended to every surface route | `policy_gate_seam.rs::every_route_asks_the_policy_engine_exactly_once`; `operation_matrix.rs::every_cell_is_reachable_or_rejected_as_stated` (every NGSI-LD route on the root and through the view root under views `all` and `restricted`: a served cell asks once with its row's clause, any other is refused before the engine); `operation_matrix.rs::every_route_of_every_surface_is_a_matrix_cell` |
| G4 | D | body size and JSON nesting on a surface route | the bounds wall over every surface (`lib.rs:487-498`) | `api_surface.rs::a_registered_surface_is_behind_the_bounds_wall`; `facade_seam_f0.rs::an_inner_request_is_counted_by_the_bounds_wall`; `security_regression.rs::oversized_body_is_rejected_before_any_parse` |
| G5 | E | a surface mounts under `/ngsi-ld/v1` or `/ex` and changes the conformance surface | `check_prefix` (`surface.rs:42-50`) and `overlaps` (`:55-57`) refuse it at startup | `api_surface.rs::a_surface_cannot_mount_outside_the_reserved_prefixes`; `surface.rs::tests::only_the_reserved_prefixes_are_mountable` |
| G6 | E | a surface feature is compiled in but serves without a view, or is linked into the vanilla binary | module rules M3 and M4 | `dev/check-vanilla-symbols.sh --self-test` (proposed); `crates/antares-broker/tests/surfaces_fail_closed.rs::a_selected_surface_without_a_view_is_closed` |
| G7 | S | a client reaches the broker without the gateway and sends a subject header itself (map S2, S26) | outside the broker: the NetworkPolicy admits only the gateway and the Portal (map S26); inside the broker the subject is only the headers the deployment named | `policy_gate_seam.rs::the_subject_carries_no_header_the_deployment_did_not_name`; deployment side: a conftest rule in joinedcontext-deployment that the broker's NetworkPolicy admits no other pod (proposed, outside this repository) |
| G8 | D | a façade over a façade recurses until the stack ends | `MAX_IN_PROCESS_CALL_DEPTH` (`bounds.rs:107`) checked in `AppState::call` (`state.rs:582-590`) | `facade_seam_f0.rs::a_facade_that_calls_itself_is_ended_by_the_broker_not_by_the_stack` |
| G9 | I | an error body leaks a hidden name, another tenant's id or the store's own text (map S23) | error mapping of section 10.3: details are fixed strings or echo the caller's own input; a 5xx detail from an inner call is replaced | `crates/antares-ops/tests/ops_errors.rs::an_inner_5xx_detail_never_reaches_the_surface` (proposed) |
| G10 | R | a write through a surface cannot be traced to the surface and view that made it | every operation a surface runs writes one `surface operation` log line with the surface name, the view id and Tenant, the operation and its status, never the subject or an argument (ADR-0020). Decided (worker-2, owner may override, T-3593): the line and not the change event, because the in-process change queue's one reader is the matcher, and a bus=nats outbox row is written inside the store's transaction, where no surface is known; options: the log line (chosen), a member on the change tuple, a `__via` member in the event's documents (one a matcher or notification could leak) | `ops_log.rs::a_surface_operation_is_logged_with_the_surface_and_the_view`; `policy_subject_on_the_wire_p6.rs::a_change_event_carries_no_subject` |

## 3. NGSI-LD view (`views`)

The STRIDE table and its entry points: [threat-model/views.md](threat-model/views.md).

## 4. OData v4.01 (`odata`)

The STRIDE table and its entry points: [threat-model/odata.md](threat-model/odata.md).

## 5. MCP (`mcp`)

The STRIDE table and its entry points: [threat-model/mcp.md](threat-model/mcp.md).

## 6. LinkML schema surface (`linkml`)

The STRIDE table and its entry points: [threat-model/linkml.md](threat-model/linkml.md).

## 7. WebSocket notifications (`ws`)

The STRIDE table and its entry points: [threat-model/ws.md](threat-model/ws.md).

## 8. Internal (in-process) Context Source registrations (`internal-csr`)

The STRIDE table and its entry points: [threat-model/internal-csr.md](threat-model/internal-csr.md).

## 9. module-rules

This section is written to be adopted verbatim by T-3443. "Red" means the named CI step exits non-zero, which fails the job `Workspace tests` of `.github/workflows/workspace.yml`; that workflow is called by `ci.yml` (pull requests into `dev`, `ci.yml:31-34`) and by `full.yml`, so every rule gates both pipelines.

### 9.1 Prerequisite: the operation layer `antares-ops`

Today no operation-layer crate exists. `antares-api` depends on `antares-store` and `antares-sql` (normal edges, Q1 below), and 17 of its non-test source files call store or SQL symbols directly (Q2: `lib.rs` 14 distinct targets, `snapshots.rs` 12, `notify.rs` 11, `temporal.rs` 9, `entities.rs` 9, `subscriptions.rs` 8, `entity_map.rs` 7, `state.rs` 6, `contexts.rs` 5, `batch.rs` 5, `csource.rs` 5, `distsub.rs` 4, `history.rs` 3, `types_attrs.rs` 2, `attrs.rs` 2, `federation.rs` 2, `repr.rs` 1). The handlers are the operations, and the seam (`gate!`, `crates/antares-api/src/lib.rs:27`; `policy`, ADR-0020) sits inside them. A surface crate today could only take `AppState`, which hands it the store (F1).

Decided (worker-2, owner may override): a new crate **`antares-ops`** is the operation layer every surface depends on, built in round 1 over `AppState::call`, because that call already runs "negotiation, the bounds wall, tenancy, the policy seam, history and notifications ... exactly once and exactly as they do for an NGSI-LD client" (`state.rs` doc comment above `call`, `:535-570`) and needs no handler to move; options: `antares-ops` over `AppState::call` now and the store calls moved behind it later (chosen), extract the 17 files' operations into `antares-ops` first (weeks of moves before any surface), let surfaces depend on `antares-api` (gives them `AppState::store`, F1).

`antares-ops` provides:

- `Ops`: a handle with private fields only (the `AppState` inside is never reachable), cloned into each surface router;
- `Ops::run(op: OperationId, request) -> response`, where `OperationId` is the closed set of `operation-matrix.yaml` row ids and the request type has no field for `NGSILD-Tenant`, `NGSILD-Snapshot` or a subject header (G2): those come from the outer request only;
- the view table (ADR-0023: `View`, its loader, its revision, resolution by id), because every non-NGSI-LD surface serves through exactly one view; and the composition hook that hands the view's predicate to `gate!` (compiled in `antares-api` only under its feature `views`, which `antares-ops` enables);
- `antares_ops::Surface`, the trait a surface implements (`router(&self, ops: Ops) -> Router`), adapted to `antares_api::ApiSurface` inside `antares-ops`;
- the error mapping of section 10.3 and the policy seam's public types re-exported as `antares_ops::policy` (`Decision`, `Filter`, `Subject`), so a surface never names `antares-api`.

Allowed edges (`[dependencies]`, normal and build):

| crate | role | may depend on (workspace crates) | must not depend on |
|---|---|---|---|
| `antares-ops` | `ops` | `antares-api` (features `views`), `antares-model`, `antares-ql` | `antares-store`, `antares-sql`, `antares-bus`, `antares-notifier`, any `surface` or `protocol` crate |
| `antares-views` | `surface`, feature `views` | `antares-ops` | everything else in the workspace |
| `antares-odata` | `surface`, feature `odata` | `antares-ops`, `odata-url`, `odata-csdl`, `odata-json`, `odata-batch`, `odata-axum`, `linkml-model` (T-3441 section 9: the CSDL model is built from the LinkML model) | everything else |
| `antares-mcp` | `surface`, feature `mcp` | `antares-ops` (protocol library `rmcp` is a registry crate) | everything else |
| `antares-linkml` | `surface`, feature `linkml` | `antares-ops`, `linkml-model` | everything else |
| `antares-ws` | `surface`, feature `ws` (ADR-0003) | `antares-ops` | everything else |
| `antares-icsr` | `surface`, feature `internal-csr` | `antares-ops` | everything else |
| `odata-url`, `odata-csdl`, `odata-json`, `odata-batch`, `odata-axum` | `protocol` | each other only as ADR-0024 decision 1 draws it (`odata-url` → `odata-csdl` behind its feature `edm`; `odata-json` → `odata-csdl`, `odata-url`; `odata-axum` → the four, behind feature `axum`) | any `antares-*` crate, in every dependency kind |
| `linkml-model` | `protocol` | none | any `antares-*` crate, in every dependency kind |

`dev-dependencies` of a surface may add `antares-ops` with its feature `test-kit` (a broker over the built-in store for integration tests) and nothing else from the workspace. `antares-broker` gains one optional dependency per surface crate, one feature per surface (the names in the table), and `full = ["views", "odata", "mcp", "linkml", "ws", "internal-csr"]`; `default` stays `["mqtt"]` (`crates/antares-broker/Cargo.toml:56`).

Every workspace package declares its role, so a crate without one is a red build rather than an unchecked one:

```toml
[package.metadata.antares]
role = "surface"            # core | ops | surface | protocol | addon
feature = "odata"           # surface only: the antares-broker feature that enables it
protocols = ["odata-url", "odata-csdl", "odata-json", "odata-batch", "odata-axum", "linkml-model"]   # surface only
```

The eleven existing crates are `core`, `examples/plugin-example` is `addon`.

### 9.2 Today's crate graph (cg)

```cypher
// Q1: workspace crate edges (42 rows at 7185e1a)
MATCH (a:Package)-[d:DEPENDS_ON]->(b:Package)
WHERE a.workspace AND b.workspace AND a.lang='rust'
RETURN a.name, b.name, d.kind ORDER BY a.name, b.name;
// Q2: antares-api source files that call store or SQL symbols (17 rows)
MATCH (f:File)-[:DEFINES]->(s:Symbol)-[c:CALLS]->(t:Symbol)
WHERE f.package='antares-api' AND NOT f.is_test AND NOT s.is_test
  AND t.package IN ['antares-store','antares-sql']
RETURN f.path, count(DISTINCT t.path) AS targets, count(c) AS edges ORDER BY targets DESC;
// Q3: unsafe in workspace code (0)
MATCH (s:Symbol) WHERE s.lang='rust' AND NOT s.external AND s.is_unsafe RETURN count(s);
```

Q1 normal edges at `7185e1a`: `antares-api` → `antares-jsonld`, `antares-matcher`, `antares-model`, `antares-notifier`, `antares-ql`, `antares-sql`, `antares-store`; `antares-broker` → `antares-api`, `antares-bus`, `antares-jsonld`, `antares-model`, `antares-plugin-example` (optional, feature `plugin-example`), `antares-sql`, `antares-store`; `antares-bus` → `antares-model`; `antares-jsonld` → `antares-model`; `antares-matcher` → `antares-jsonld`, `antares-model`, `antares-ql`; `antares-notifier` → `antares-jsonld`, `antares-model`; `antares-plugin-example` → `antares-api`, `antares-model`, `antares-notifier`, `antares-ql`, `antares-store`; `antares-ql` → `antares-jsonld`, `antares-model`; `antares-sql` → `antares-model`, `antares-ql`, `antares-store`; `antares-store` → `antares-model`, `antares-ql`; `antares-wasm` → `antares-api`, `antares-jsonld`, `antares-sql`. The other rows are `dev` edges.

### 9.3 Rule M1: each surface is its own crate behind its own cargo feature; a protocol library has no dependency on any `antares-*` crate

- **Enforced by:** new script `dev/check-module-rules.py`, step "Module rules" in `.github/workflows/workspace.yml`, placed right after the step "No addon in the shipped build" (`workspace.yml:258`).
- **Input:** `cargo metadata --format-version 1 --locked --no-deps`. It reads `packages[]` (`name`, `manifest_path`, `metadata.antares`, `features`) and each package's `dependencies[]` (`name`, `kind` = `null` | `"dev"` | `"build"`, `optional`, `uses_default_features`, `path`); `name` is the real package name, so a `package = "..."` rename cannot hide an edge.
- **Red when:** a workspace package has no `metadata.antares.role`; a `protocol` package has any dependency, of any kind, whose name starts with `antares-`; a `surface` package is not an `optional = true` dependency of `antares-broker`; its `metadata.antares.feature` is not a key of `antares-broker`'s `features`, or that feature does not contain `dep:<crate>`, or two surfaces name the same feature; the transitive closure of `antares-broker`'s `default` feature contains a surface feature or `dep:` of a surface, `ops` or `protocol` crate.
- **Self-test:** `dev/check-module-rules.py --self-test` runs every check against a synthetic metadata document with exactly one violation per red condition and fails unless each is reported (the same discipline as `dev/check-no-addon.sh:32-37`); the step runs `--self-test` first, then the real check.
- **Each feature builds and tests alone:** the feature matrix of T-3465 runs `cargo test -p antares-broker --no-default-features --features <f>` per surface feature; that is T-3465's job, M1 only guarantees the features exist one-to-one.

### 9.4 Rule M2: a surface crate depends only on the operation layer, the policy seam (through `antares-ops`) and its protocol library; never on `antares-store` or SQL

- **Enforced by:** the same `dev/check-module-rules.py`, same step, plus a symbol-level query for reviewers and for the T-3446 gate.
- **Input:** as M1, plus a read of `crates/antares-ops/src/**/*.rs`.
- **Red when:** a `surface` package's normal or build dependencies inside the workspace are not a subset of `{antares-ops} ∪ metadata.antares.protocols`, or a listed protocol is not a `protocol` package; its dev-dependencies inside the workspace are anything other than `antares-ops`; the `ops` package has a normal or build dependency on `antares-store`, `antares-sql`, `antares-bus`, `antares-notifier` or any `surface`/`protocol` crate; a file under `crates/antares-ops/src/` matches `pub use antares_(store|sql)`, `pub use antares_api::(AppState|state)`, or declares a `pub` field or a `pub fn` returning `AppState` (the re-export that would hand F1 back).
- **Self-test:** as M1, one synthetic violation per condition, and a scratch file under a temporary copy of `crates/antares-ops/src/` for the grep half.
- **Symbol-level check** (run with `cg build --reuse` after any move; the GOAL "cg query" form of the rule):

```cypher
// M2: no surface symbol touches a store/SQL symbol or AppState's store fields (expected: 0 rows)
MATCH (s:Symbol)-[r:CALLS|REFERENCES]->(t:Symbol)
WHERE s.package IN ['antares-views','antares-odata','antares-mcp','antares-linkml','antares-ws','antares-icsr']
  AND (t.package IN ['antares-store','antares-sql']
       OR t.path IN ['antares_api::state::AppState::store','antares_api::state::AppState::temporal'])
RETURN s.package, s.path, t.path, r.line;
// M1: no protocol crate depends on an antares-* crate (expected: 0 rows)
MATCH (a:Package)-[d:DEPENDS_ON]->(b:Package)
WHERE (a.name STARTS WITH 'odata-' OR a.name = 'linkml-model') AND b.name STARTS WITH 'antares-'
RETURN a.name, b.name, d.kind;
```

Whether `cg` can be installed on a GitHub runner: unknown. Until it can, the two queries run in the T-3446 gate and in review, and `dev/check-module-rules.py` is the CI enforcement.

### 9.5 Rule M3: feature off = no route registered and no code linked

- **Enforced by:**
  1. `dev/check-no-addon.sh`, `FORBIDDEN` (`dev/check-no-addon.sh:22`) extended with `antares-ops|antares-views|antares-mcp|antares-linkml|antares-ws|antares-icsr|odata-|linkml-model|rmcp|tungstenite` (`antares-odata` is already in it). It runs in `workspace.yml:258`, `full.yml:365` and `build-image.yml:52`; its self-test (`:34`) is changed to build the tree with `--features full` as well as `plugin-example`, so the new names must match a full tree.
  2. New script `dev/check-vanilla-symbols.sh`, step "Vanilla binary links no surface" in `workspace.yml`, after the unit-test steps (it reuses their build cache). It runs `cargo build --locked -p antares-broker` (the `dev` profile, which is not stripped; release is, F6), then `nm --defined-only --demangle target/debug/antares` and greps for the surface and protocol crate paths (`antares_ops::`, `antares_views::`, `antares_odata::`, `antares_mcp::`, `antares_linkml::`, `antares_ws::`, `antares_icsr::`, `odata_url::`, `odata_csdl::`, `odata_json::`, `odata_batch::`, `odata_axum::`, `linkml_model::`, `rmcp::`, `tungstenite::`). Self-test: the same grep must match a `cargo build --locked -p antares-broker --features full` binary. It then starts the vanilla binary with `ANTARES_API_SURFACES=admin,odata` and requires a non-zero exit naming "unknown surface" (`crates/antares-broker/src/main.rs:478-486`).
  3. The same script in mode `--audit-info <binary>` in `build-image.yml` on the image's `/antares`: the Dockerfile builds with `cargo auditable` (`Dockerfile:37`), which embeds the package list, and `rust-audit-info` 0.5.4 (MIT OR Apache-2.0) prints it; no surface, ops or protocol crate may appear. This checks the shipped artefact itself, which is stripped.
  4. The route half: test `crates/antares-broker/src/main.rs::api_surface_tests::the_vanilla_shelf_is_admin_alone` (proposed, `#[cfg(not(any(feature = ...)))]` over the surface features), beside the existing `api_surface_tests::an_unknown_surface_is_fatal_and_lists_the_shelf` (`main.rs:1613`): `SURFACE_SHELF` (`main.rs:455-461`) is exactly `["admin"]`, and `GET` on one path of each surface prefix answers 404 through `router()`.
- **Red when:** a forbidden name is in the default `cargo tree`; a forbidden symbol is in the vanilla debug binary; the vanilla binary accepts a surface name; a forbidden package is in the image's embedded list; the shelf test fails; or any self-test fails to match the full build.

### 9.6 Rule M4: feature on but no view or policy configured = the surface does not start; it never serves allow-all

- **Mechanism:** a selected surface opens only with a valid configuration: for `views`, `odata`, `mcp`, `linkml` and `ws` at least one view that loaded (ADR-0023 rule 10), for `internal-csr` at least one export grant (C1). Without one the surface is **closed**: no route under its prefix is served (404, the answer for any unknown path), `/q/health` lists it as `{"state": "closed", "reason": "..."}`, and it opens when the view table reload (ADR-0023 rule 10) brings a valid view. Decided (worker-2, owner may override): closed-and-reported rather than a process exit, because views are written through `/q` of the running broker, so a fresh deployment that exited would have no way to receive its first view; options: closed and reported (chosen), refuse to start the process, serve the surface with an empty view. No code path builds an allow-all view: `View` has no `Default`, its selector needs at least one type, and no environment variable produces a view. A surface named in `ANTARES_API_SURFACES` that this binary was not built with stays fatal (`main.rs:482`).
- **Enforced by:** test file `crates/antares-broker/tests/surfaces_fail_closed.rs` (proposed), run by the step `dev/unit-junit.sh all-features --workspace --all-features` of `workspace.yml`, with tests `a_selected_surface_without_a_view_is_closed` (every surface selected, empty view table: every route of every surface router answers 404, health says closed), `a_surface_opens_when_its_first_view_loads` and `closes_again_when_its_last_view_is_removed`, `a_view_that_failed_validation_does_not_open_a_surface`, and `there_is_no_allow_all_view` (a `compile_fail` doc test that `View::default()` does not exist, plus a check that an empty selector is refused at load).
- **Red when:** any of those tests fails.

### 9.7 What the rules do not cover

- That the handlers behind `AppState::call` themselves stay free of a second data path: that is ADR-0020's route walk (`policy_gate_seam.rs::every_route_asks_the_policy_engine_exactly_once`) extended by G3.
- Moving the store calls of the 17 files behind `antares-ops` (a later round; rule M2 does not change when it happens, because surfaces never had the edge).

## 10. Code rules for every new crate

Applies to `antares-ops`, the six surface crates and the six protocol crates of section 9.1.

### 10.1 The `[lints]` table each new crate carries

Cargo's `[lints] workspace = true` cannot be combined with crate-level entries, and the workspace table sets only `unsafe_code = "forbid"`, `unexpected_cfgs`, `unwrap_used = "warn"` and an allow (`Cargo.toml:155-166`, F9). So each new crate carries its own complete table instead of inheriting, and the workspace table is left as it is for the existing crates (raising it is T-3468's):

```toml
[lints.rust]
unsafe_code = "forbid"
missing_docs = "deny"
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(tokio_unstable)"] }

[lints.clippy]
pedantic = { level = "deny", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
arithmetic_side_effects = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
print_stdout = "deny"
print_stderr = "deny"

[lints.rustdoc]
broken_intra_doc_links = "deny"
```

Plus, per crate:

- `#![forbid(unsafe_code)]` at the top of `src/lib.rs` as well (GOAL point 8 names the attribute; the lint table alone is invisible to a reader of the source and to a crate copied out of the workspace).
- A crate-local `clippy.toml` with `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`, `allow-panic-in-tests = true`, `allow-indexing-slicing-in-tests = true`, so `--all-targets` holds the request-path rules without forcing them on `#[test]` bodies. All four keys exist in the clippy shipped with rustc 1.99.0 (`strings` of the local `clippy-driver`; the repository pins `channel = "stable"` in `rust-toolchain.toml`). No such key exists for `arithmetic_side_effects` (none in that list), so test code in these crates uses `checked_*`/`saturating_*` as well, or a test module writes `#[expect(clippy::arithmetic_side_effects, reason = "...")]`; Decided (worker-2, owner may override): checked arithmetic in tests too, because an `expect`/`allow` is what the GOAL rule "no `allow(...)`" forbids; options: checked arithmetic (chosen), `#[expect]` with a reason.
- `overflow-checks = true` for the new crates in release: `[profile.release.package.<crate>] overflow-checks = true` in the root `Cargo.toml`, one block per new crate (GOAL point 8 names this or checked arithmetic; with `arithmetic_side_effects` denied both hold).
- No `allow(...)` anywhere in these crates. `needless_borrows_for_generic_args` is allowed workspace-wide because of a clippy memory blow-up on the `antares-api` test target (`Cargo.toml:164-166`); the new crates do not inherit that allow, so the lint keeps its default level there (warn, red under `-D warnings`).

CI: the clippy step becomes `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` (`workspace.yml:150` today has no `--all-features`, F7); `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items` already runs daily in `strict.yml` (job `hygiene`) and moves to `workspace.yml` for the new crates (`-p` each); `cargo +nightly miri test` (`strict.yml:240`, today `-p antares-model -p antares-ql -p antares-matcher`) adds `-p odata-url -p odata-csdl -p odata-json -p odata-batch -p linkml-model` (not `odata-axum`, which is an HTTP adapter) (pure parsers and emitters, GOAL point 8). Public items carry a doc example that compiles (`cargo test --doc`, already in `workspace.yml`).

### 10.2 Parser limits

Every parser takes its limits as an argument, with no hidden default on the request path (odata-library.md section 3: "Every limit is a field of a `Limits` struct passed by the caller"); counters use checked arithmetic and return a `LimitExceeded` error, never wrap or panic; size is checked before parse (the bounds wall's own rule, `bounds.rs:2-8`). Each parser has a fuzz target in `fuzz/fuzz_targets/` (today: `expand_entity`, `geo_params`, `parse_q`, `scope_q`); proposed: `odata_url_parse`, `odata_batch_multipart`, `odata_batch_json`, `odata_json_body`, `odata_csdl_read` (if CSDL reading is kept, odata-library.md 3.2), `mcp_cursor`, `mcp_tool_arguments` (MCP rule 14), `ws_client_message`, `view_config_load`.

### 10.3 Errors per surface, without leaking hidden names

One mapping, implemented once in `antares-ops` and rendered per surface. A message is either a fixed string per condition or, on the NGSI-LD view only, an echo of the caller's own input (bounded to 256 bytes, control characters escaped); the OData error body echoes no request value at all and has no `innererror` (ADR-0024 decision 5, T-3441 S3), and an MCP refusal's text is status and `title` only (MCP rule 1). It never contains a stored value, another Entity's id, a type or Attribute name the caller did not send, the view's predicate, a tenant name the caller did not send (V18), or the text of an inner 5xx (replaced by a fixed string and a request id; map S23). An engine's `Deny(String)` reason reaches the client as ADR-0020 states ("Fixed strings: they reach the client in a ProblemDetails `detail`"); a view's own refusals use fixed strings. "Hidden" and "absent" produce identical bytes wherever the table says "as absent".

| condition | NGSI-LD view (ProblemDetails, TS 104 176 6.3.2 / 6.3.3) | OData (JSON error object, matrix 7.1 cites JSON Format 21.1) | MCP (JSON-RPC 2.0) | WebSocket | LinkML |
|---|---|---|---|---|---|
| out-of-view or absent Entity | 404 `ResourceNotFound` (ADR-0023 rule 3) | 404, `code` `NotFound` | tool result `isError: true` with status and `title`; resource: `-32602` as an unknown resource (MC10) | error message, ProblemDetails `type` only | n/a |
| hidden or unknown name in the request | as absent: `q` selects nothing (rule 2) | 400, `code` `UnknownProperty`, same bytes for hidden and never-existing (O1) | `-32602`, same bytes (MC1) | error message `BadRequestData` | n/a |
| operation not allowed by the view | 405 + `Allow`, or 422 `OperationNotSupported` (rule 4) | 405 + `Allow`, or 501 for matrix 7.1 features | tool absent from `tools/list`; a call answers as unknown tool | error message `OperationNotSupported` | 405 + `Allow` |
| engine `Deny`, write leaving the view | 403 `urn:antares:error:AccessDenied` (ADR-0020) | 403, `code` `Forbidden` | `isError: true` with status 403 and `title` | error message `AccessDenied` | 403 |
| malformed input | 400 `BadRequestData` / `InvalidRequest` | 400, `code` `BadRequest`, `target` = the option name | `-32600` (envelope) or `-32602` (arguments) | close 1007 or 1008 | 400 |
| a limit exceeded | 413 / 414 bare, 400, 403 `TooComplexQuery` / `TooManyResults` (bounds table, section 2.3) | the same status, `code` `LimitExceeded`, `message` names the limit, never the value that was hidden | `isError: true` naming the argument to narrow (MCP section 8) | close 1009 | 403 `TooManyResults` |
| surface closed (M4) | 404 | 404 | 404 (HTTP) | upgrade 404 | 404 |
| a remote `@context` the request names cannot be retrieved (`Refusal::ContextNotAvailable`, T-3947) | 504 `LdContextNotAvailable` (TS 104 176 6.3.2, as the root), fixed detail: never the URL or the fetch error | 504, `code` `InternalError` (`read.rs` `refused`) | tool result `isError: true` with status 504 and `title` | error message with status 504 and the same ProblemDetails | 504 `LdContextNotAvailable` |
| inner failure | 500 `InternalError`, fixed detail and request id | 500, `code` `InternalError` | `-32603`, fixed message | close 1011 | 500 |

The error codes in the OData column are proposals (the matrix: "OData leaves the code service-defined"). Tests: G9, O20, MC18, LK8 and `views_errors.rs::views_errors_carry_no_hidden_name_and_no_inner_detail` (proposed).

## 11. Supply chain

### 11.1 Gates

| gate | today | change | CI step | red when |
|---|---|---|---|---|
| `cargo deny` 0.20.2 | `check licenses advisories` per run of `workspace.yml` (`:209-215`); `check all` weekly in `advisories.yml` (F5) | per run: `cargo deny --log-level warn check licenses advisories bans sources`; `deny.toml` `[bans]` gains `deny = [{ name = "openssl-sys" }, { name = "native-tls" }, { name = "aws-lc-sys" }]`, the crates the workspace comments already keep out ("aws-lc-sys (C + cmake) never enters the build", `Cargo.toml:70-73`; one rustls stack on ring, `Cargo.toml:49-54`) | `workspace.yml` step "cargo-deny (licenses + advisories)" renamed "cargo-deny" | a licence off the allow list (`deny.toml:4-17`), an advisory not in `deny.toml:21-40`, a yanked crate, a wildcard version, a banned crate, an unknown registry or git source |
| `cargo audit` 0.22.2 (Apache-2.0 OR MIT, crates.io 2026-06-05) | not run; the Dockerfile notes "`cargo audit bin /antares` can then verify a shipped broker" (`Dockerfile:16-19`) | `cargo audit --deny warnings` on `Cargo.lock`, and `cargo audit bin` on the vanilla and the full image binaries; the ignore list lives in `deny.toml` alone and is passed as `--ignore <id>` flags generated from it by a new `dev/advisory-ignores.py`, so the two tools cannot drift | `workspace.yml` new step "cargo-audit"; `build-image.yml` new step "cargo audit bin" | an advisory or warning (unmaintained, yanked) not on the `deny.toml` ignore list |
| `cargo machete` 0.9.2 (MIT) | daily in `strict.yml` job `hygiene` (`:41-44`), weekly in `xray.yml` | also per run in `workspace.yml` (no build needed) | `workspace.yml` new step "cargo-machete" | an unused dependency in any crate |
| CycloneDX SBOM, `cargo-cyclonedx` 0.5.9 (Apache-2.0, crates.io 2026-03-19) | none; an SPDX SBOM of the released image by syft on `v*` tags (`full.yml:417-421`) | two SBOMs per build of the broker: vanilla (default features) and full (`--features full`), JSON, from `crates/antares-broker/Cargo.toml`; uploaded as artefacts, attached to the GitHub release beside the syft SPDX file; the vanilla SBOM is grepped for the M3 forbidden names | `workspace.yml` new step "SBOM (vanilla and full)"; `full.yml` job `github-release` attaches them | `cargo cyclonedx` fails, either file is missing or empty, or the vanilla SBOM names a surface, ops or protocol crate |
| `default-features = false` | not checked | `dev/check-module-rules.py` (section 9) also checks that every registry dependency of an `ops`, `surface` or `protocol` crate has `uses_default_features == false`, unless it is listed in `[package.metadata.antares] default-features-ok = [...]` with a reason | `workspace.yml` step "Module rules" | a default-on registry dependency without a listed reason |
| tool pins | `cargo-deny` pinned (`V=0.20.2`); `cargo install cargo-chef cargo-auditable --locked` without versions (`Dockerfile:20`) | pin `cargo-auditable@0.7.7` (crates.io 2026-10-02) and `cargo-chef`, and every new tool above, by exact version | the steps above, `Dockerfile` | n/a (a pin, reviewed) |

The exact command-line flags of `cargo-cyclonedx` 0.5.9 for selecting features and the output file name were not verified (the tool was not run): unknown, to be read from its `--help` when the step is written.

### 11.2 Every new dependency the surfaces need

Allow list: `deny.toml:4-17` (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, EUPL-1.2, ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0). Versions and licences are the latest stable on crates.io read on 2026-10-10 unless the row says otherwise; "`unsafe` lines" is a grep of the crate tarball's `src/` for the word `unsafe` (comment lines not removed for the two crates grepped in this task, so the counts are upper bounds; the OData rows are copied from odata-library.md section 6).

| crate | version | licence | allowed | in `Cargo.lock` | `unsafe` | used by | features (`default-features = false`) | reason |
|---|---|---|---|---|---|---|---|---|
| `rmcp` | 3.5.1 (2026-10-05) | Apache-2.0 | yes | no | 0 lines in `rmcp-3.5.1/src`; no `forbid(unsafe_code)` attribute | `antares-mcp` | `server`, `transport-streamable-http-server` (MCP 6.3); its `default` is `base64`, `macros`, `server` | the MCP protocol library; exact pin `=3.5.1` (MCP 6.3: a minor release already changed which revision `LATEST` names) |
| `rmcp-macros` | 3.5.1 | Apache-2.0 | yes | no | not grepped | only if `macros` is enabled | not enabled by the chosen feature set | listed because `rmcp`'s default pulls it |
| `schemars` | 1.2.2 (2026-07-27) | MIT | yes | no | not grepped | `rmcp` feature `server` | as `rmcp` sets | JSON Schema of tool inputs |
| `pastey` | 0.2.3 | MIT OR Apache-2.0 | yes | no | not grepped | `rmcp` feature `server` | n/a (proc-macro) | pulled by `rmcp` |
| `sse-stream` | `^0.2.4` required by `rmcp` (0.2.4 to 0.2.6 and 0.3.0 all MIT OR Apache-2.0) | MIT OR Apache-2.0 | yes | no | not grepped | `rmcp` feature `server-side-http` | as `rmcp` sets | streamed responses (`subscriptions/listen`) |
| `tokio-stream` | 0.1.19 | MIT | yes | yes, 0.1.19 (`Cargo.lock:3671`) | not grepped | `rmcp` | as `rmcp` sets | |
| `hmac` | 0.13.0 | MIT OR Apache-2.0 | yes | yes, 0.13.0 (`Cargo.lock:1372`) | not grepped | `antares-mcp` (`requestState`, MCP D-11) | default | HMAC-SHA256 of `requestState`; directly or through `rmcp`'s `request-state` feature: T-3442 decides |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | yes | yes, 0.10.9 and 0.11.0 (`Cargo.lock:3078`, `:3089`) | not grepped | as `hmac` | default | as `hmac` |
| `tokio-tungstenite` | 0.29.0 (latest is 0.30.0) | MIT | yes | no | 0 lines | `antares-ws` directly, `default-features = false`, feature `handshake` (T-3704) | `handshake` only | WebSocket upgrade and framing |
| `tungstenite` | 0.29.0 | MIT OR Apache-2.0 | yes | no | 5 lines: `src/utf8.rs:59`, `:99` and `src/protocol/frame/utf8.rs:20`, `:28` (`from_utf8_unchecked`), `src/protocol/frame/mask.rs:26` (`align_to_mut`) | as above | as axum sets | the unavoidable `unsafe` GOAL point 8 asks to name; limits set explicitly (F8, W3, W4) |
| `sha1` | `^0.10` required by tungstenite's `handshake` feature (0.11.0 in the lock at `Cargo.lock:3061`; 0.10.x licence not read) | 0.11.0: MIT OR Apache-2.0 | yes for 0.11.0; 0.10.x unknown | 0.11.0 | not grepped | tungstenite `handshake` (`derive_accept_key`, the `Sec-WebSocket-Accept` hash) | as tungstenite sets | |
| `quick-xml` | 0.42.0 (2026-08-22) | MIT | yes | no | `#![forbid(unsafe_code)]` (odata-library.md 6) | `odata-csdl` | `serialize` only if reading CSDL is kept | CSDL XML writer for `$metadata` |
| `memchr` | 2.8.3 | Unlicense OR MIT | yes (MIT) | yes (`Cargo.lock:1925`) | yes, SIMD (odata-library.md 6) | `odata-batch`, `quick-xml` | `std` | multipart boundary search |
| `serde`, `serde_json` | 1.0.229, 1.0.151 | MIT OR Apache-2.0 | yes | yes | odata-library.md 6 | every protocol crate | `derive` / `std` | |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | yes | yes (`Cargo.lock:2239`) | 4 lines (odata-library.md 6) | `odata-url` | default | URL decoding |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | yes | yes (`Cargo.lock:3516`) | not counted | every new crate | default | error types |
| `indexmap` | 2.14.2 (lock 2.14.0, `Cargo.lock:1736`) | Apache-2.0 OR MIT | yes | yes | not counted | `odata-json`, `linkml-model` | `std` | member order in written documents |
| `axum`, `http` | 0.8.9, 1.5.0 (odata-library.md 6) | MIT; MIT OR Apache-2.0 | yes | yes (`Cargo.lock:378`, `:1380` per odata-library.md 6) | not counted | `odata-axum` | axum without defaults, as `crates/antares-api/Cargo.toml:52` already does | the HTTP adapter of ADR-0024 decision 1 |
| `nom` | 8.0.0 | MIT | yes | yes | 4 lines (odata-library.md 6) | none in round 1 | | fallback parser only; the chosen parser is hand-written (odata-library.md 6) |
| `geo` | 0.33.1 | MIT OR Apache-2.0 | yes | yes, 0.33.1 (`antares-ql` already links it) | 2 matches of `unsafe` in `geo-0.33.1/src` | `antares-residual` (T-3756) | `default-features = false`, as the workspace sets | DE-9IM `relate` of the residual's spatial relations, the matrix `antares-ql` matches `georel` with |
| `serde-saphyr` | 1.3.0 (2026-09-16) | MIT OR Apache-2.0 | yes | no | forbids it (T-3441 section 9) | `linkml-model` dev-dependency only | default | YAML parse in the round-trip property test (LK3); no YAML parser at runtime (section 6, T-3441 section 9); `#![forbid(unsafe_code)]` per T-3441 section 9 |
| `proptest` | 1.11.0 | MIT OR Apache-2.0 | yes | no | not counted | dev-dependency of every new crate | default | property tests (GOAL point 5) |
| `arbitrary` | 1.5.0 | MIT OR Apache-2.0 | yes | no | not counted | `fuzz/` only | `derive` | fuzz input types; `fuzz/` is outside the root `cargo deny` (excluded, `Cargo.toml:21`), so its `libfuzzer-sys` (NCSA, not on the list) stays as today |

Tools, not dependencies: `cargo-deny` 0.20.2 (MIT OR Apache-2.0), `cargo-audit` 0.22.2, `cargo-machete` 0.9.2, `cargo-cyclonedx` 0.5.9, `cargo-auditable` 0.7.7 (MIT OR Apache-2.0), `rust-audit-info` 0.5.4 (MIT OR Apache-2.0, last release 2024-05-08).

## 12. Decisions, unknowns and counts

### 12.1 Decisions taken here (worker-2, owner may override)

| # | decision | section |
|---|---|---|
| D1 | `antares-ops` over `AppState::call` is the operation layer; surfaces depend on it alone | 9.1 |
| D2 | the view table and the gate composition hook live in `antares-ops` / `antares-api` feature `views`, so every surface serves through one view | 9.1 |
| D3 | M4 closes an unconfigured surface (404, reported in `/q/health`) instead of exiting the process | 9.6 |
| D4 | every new crate carries its own complete `[lints]` table; the workspace table is not raised here | 10.1 |
| D5 | checked arithmetic in test code too (no `#[expect]`) | 10.1 |
| D6 | a Replace through a view is refused when the stored Entity holds hidden Attributes | V11 |
| D7 | a Relationship target must be visible, and an absent target answers alike | V13 |
| D8 | `MAX_Q_NODES` counts the caller's `q` alone; the view predicate is bounded at load | V23 |
| D9 | reaching `MAX_JOIN_LOOKUPS` in an OData `$expand` fails with 400, never a partial answer | O6 |
| D15 | `Cache-Control: private, no-cache` on `$metadata` and the LinkML documents (amends T-3441 7.4) | O16, LK2 |
| D10 | a JSON-RPC batch array is refused with `-32600` | MC6 |
| D11 | (taken by T-3441 section 9, adopted here) the LinkML surface writes YAML with its own emitter and parses none | 6 |
| D12 | an oversized LinkML schema is refused when it is written | LK7 |
| D13 | internal CSR needs an export grant held by the target tenant, checked at registration and on every forward | C1 |
| D14 | an export grant is read-only unless it says read-write | C6 |

### 12.2 Unknown or not done

- `cg` on a GitHub runner: unknown; the M1/M2 Cypher runs in review and in T-3446 until it is (9.4).
- Whether MCP 2026-07-28 allows JSON-RPC batching: not re-read; MC6 refuses arrays either way.
- `cargo-cyclonedx` 0.5.9 feature and output flags: not verified (11.1).
- `sha1` 0.10.x licence: not read; `unsafe` not grepped for `rmcp-macros`, `schemars`, `pastey`, `sse-stream`, `tokio-stream`, `hmac`, `sha2`, `sha1`, `serde-saphyr`.
- No 414 rule found in TS 104 176 clause 6; the bounds wall's 414 rests on CIM 009 6.3.4 comments (2.3).
- `docs/analysis/mcp-surface.md` is a draft; MCP rule 2's `antares-api` edge conflicts with M2 (section 5 note).
- `remote_notify` and `pre_notify` (matrix section 8 item 13) were not traced here either; WebSocket and MCP delivery assume `deliver_as` (W5, MC17).
- Every test marked "(proposed)" does not exist yet. The STRIDE test cells name 168 distinct tests, fuzz targets and scripts: 30 exist (each checked to be a unique `fn` under `crates/`), 138 are proposed (counted by a script over the test column); proposed names taken from ADR-0023, T-3441 and T-3442 are marked with their source and are listed without a file where the source gave none.

### 12.3 Row counts

`python3 dev/threat-tests.py` prints the number of STRIDE rows and of the tests they name; a
hand-kept count went stale with every new row and was the line every surface branch edited.

## 13. Related

- `tasks/GOAL-antares-surfaces.md` points 5, 6 and 8; tasks T-3443 (architecture ADR, adopts section 9), T-3444 (test plan, takes every test name above), T-3445 (files the round), T-3446 (gate), T-3462 to T-3468 (baseline), T-3470 (join visibility).
- `docs/analysis/operation-matrix.md`, `docs/analysis/gateway-to-broker-map.md`, `docs/analysis/odata-library.md`, `docs/analysis/mcp-surface.md`, `docs/analysis/cim009-to-ts104175.md`
- `docs/adr/ADR-0003-websocket-deferred.md`, `ADR-0010-egress-allow-private-by-default.md`, `ADR-0011-tenant-specific-context-source-alias.md`, `ADR-0014-extension-hooks-model.md`, `ADR-0020-policy-seam.md`, `ADR-0023-endpoint-views.md`
- `crates/antares-api/src/bounds.rs`, `src/surface.rs`, `src/state.rs`, `src/policy.rs`, `src/lib.rs`; `crates/antares-broker/src/main.rs`; `dev/check-no-addon.sh`; `.github/workflows/workspace.yml`, `full.yml`, `strict.yml`, `advisories.yml`, `build-image.yml`, `xray.yml`; `deny.toml`; `Dockerfile`
