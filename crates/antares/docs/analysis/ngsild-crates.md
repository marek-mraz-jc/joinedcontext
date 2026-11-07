# NGSI-LD crates reusable outside Antares: what each one pulls in, role `ngsild` and rule M3

Sources: Antares-NGSI-LD-Context-Broker origin/dev `3232418`, code graph built at that commit (`cg build --fg`), and `cargo metadata --format-version 1 --locked --no-deps` at origin/dev `7d74f43` (branch `agent/worker-2/T-3910`). Task T-3910, owner order 2026-10-11 ("optimize the crates for NGSI-LD so they can be used somewhere else"). Follow-ups: T-3911 (the JSON-LD loader behind a `fetch` feature), T-3912 (publish-ready), T-3913 (publish).

## 1. Decisions

- Role `ngsild` (`dev/check-module-rules.py`, `ROLES`): a crate of pure NGSI-LD code another project can embed. Rule M3: it depends inside the workspace only on `ngsild` crates and outside it only on `NGSILD_EXTERNAL` (`serde`, `serde_json`, `thiserror`, `chrono`, `regex`, `geo*`, `unicode-*`, `uuid`); any other crate is an optional dependency its `default` feature does not turn on. Build dependencies count, dev dependencies do not.
- Names: Decided (supervisor, owner may override): the `antares-*` package names and `[lib] name` stay; publishing as `ngsild-*` is T-3913's decision, once crates.io ownership is known. Options were rename now, rename at publish, never publish.
- Decided (worker-2, owner may override): `antares-store` stays `core`. Its traits are the broker's driver seam: `Kind` names the broker's own document kinds (Snapshot, EntityMap, DistSub, DeadLetter, View), `async-trait` is outside the allowlist, and it depends on `antares-jsonld` and `antares-ql`. A storage-trait crate for other projects would be a new, smaller crate, not this one; nothing asks for it yet. Options: move it after T-3911 (still the broker's kinds), split a trait crate now (no consumer), keep it core (chosen).
- Decided (worker-2, owner may override): `antares-residual` passes M3 today but stays role `ops` in this task. Role `ops` holds it to the 100 % line and branch coverage gate (`dev/coverage-gate.py`, roles ops, surface, protocol); moving it to `ngsild` would drop it from that gate. `ngsild` joins the gate in T-3912, which makes the crates publish-ready, and residual moves then. `antares-model` measures 99.54 % lines and 93.33 % branches today (`EntityId::as_str` has no caller and no test, two branches of `EntityId::new`'s scheme check, three assertion branches in its tests), so it cannot join the gate in this task without growing it. Done in T-3912: `ngsild` is in the gate's roles, antares-model covers 100 % of lines and branches, and antares-residual carries role `ngsild`.

## 2. Where each candidate stands

| Crate | Role after T-3910 | Passes M3 | Blocking | Task |
|---|---|---|---|---|
| antares-model | ngsild | yes | | |
| antares-residual | ngsild (T-3912) | yes | | T-3912 moved it once `ngsild` joined the coverage gate |
| antares-ql | ngsild (T-3911) | yes | | |
| antares-matcher | ngsild (T-3911) | yes | | |
| antares-jsonld | ngsild (T-3911) | yes | the network loader sits behind the non-default `fetch` feature (section 7) | |
| antares-store | core | no | `async-trait`, jsonld, ql; stays core (section 1) | |

`antares-residual` has no workspace dependent at all (no `Cargo.toml` names it outside its own crate, and query 3 finds no caller), so its move changes no other crate.

## 3. Dependencies (query 1)

```cypher
MATCH (a:Package {workspace:true})-[d:DEPENDS_ON]->(b:Package) WHERE a.name IN ['antares-model','antares-ql','antares-matcher','antares-residual','antares-jsonld','antares-store'] AND d.kind<>'dev' RETURN a.name, b.name, b.workspace, d.kind ORDER BY a.name, b.name;
```

```
38 rows, 21 ms
a.name | b.name | b.workspace | d.kind
antares-jsonld | antares-model | True | normal
antares-jsonld | chrono | False | normal
antares-jsonld | futures-util | False | normal
antares-jsonld | gloo-timers | False | normal
antares-jsonld | moka | False | normal
antares-jsonld | reqwest | False | normal
antares-jsonld | rustls | False | normal
antares-jsonld | send_wrapper | False | normal
antares-jsonld | serde_json | False | normal
antares-jsonld | tokio | False | normal
antares-jsonld | uuid | False | normal
antares-jsonld | web-time | False | normal
antares-matcher | antares-jsonld | True | normal
antares-matcher | antares-model | True | normal
antares-matcher | antares-ql | True | normal
antares-matcher | chrono | False | normal
antares-matcher | serde_json | False | normal
antares-model | serde | False | normal
antares-model | serde_json | False | normal
antares-model | thiserror | False | normal
antares-ql | antares-jsonld | True | normal
antares-ql | antares-model | True | normal
antares-ql | geo | False | normal
antares-ql | geo-types | False | normal
antares-ql | geojson | False | normal
antares-ql | regex | False | normal
antares-ql | serde | False | normal
antares-ql | serde_json | False | normal
antares-residual | geo | False | normal
antares-residual | regex | False | normal
antares-residual | serde_json | False | normal
antares-residual | unicode-normalization | False | normal
antares-store | antares-jsonld | True | normal
antares-store | antares-model | True | normal
antares-store | antares-ql | True | normal
antares-store | async-trait | False | normal
antares-store | chrono | False | normal
antares-store | serde_json | False | normal
```

## 4. Public API (query 2)

```cypher
MATCH (p:Package)-[:CONTAINS]->(:File)-[:DEFINES]->(s:Symbol) WHERE p.name IN ['antares-model','antares-ql','antares-matcher','antares-residual','antares-jsonld','antares-store'] AND s.visibility='pub' AND NOT s.is_test RETURN p.name, s.kind, count(*) ORDER BY p.name, s.kind;
```

```
44 rows, 38 ms
p.name | s.kind | COUNT_STAR()
antares-jsonld | Constant | 8
antares-jsonld | Field | 18
antares-jsonld | Function | 31
antares-jsonld | Method | 29
antares-jsonld | Module | 4
antares-jsonld | StaticMethod | 7
antares-jsonld | Struct | 7
antares-jsonld | TypeAlias | 5
antares-matcher | Function | 6
antares-model | Constant | 11
antares-model | Enum | 1
antares-model | Field | 13
antares-model | Function | 8
antares-model | Method | 6
antares-model | Module | 3
antares-model | StaticMethod | 4
antares-model | Struct | 5
antares-ql | Constant | 10
antares-ql | Enum | 4
antares-ql | Field | 22
antares-ql | Function | 26
antares-ql | Method | 8
antares-ql | Module | 5
antares-ql | StaticMethod | 2
antares-ql | Struct | 5
antares-ql | TypeAlias | 2
antares-residual | Constant | 9
antares-residual | Enum | 9
antares-residual | Field | 5
antares-residual | Function | 4
antares-residual | Method | 6
antares-residual | StaticMethod | 5
antares-residual | Struct | 4
antares-store | Constant | 2
antares-store | Enum | 3
antares-store | Field | 61
antares-store | Function | 9
antares-store | Method | 23
antares-store | Module | 1
antares-store | StaticMethod | 1
antares-store | Struct | 12
antares-store | Trait | 4
antares-store | TraitMethod | 36
antares-store | TypeAlias | 4
```

## 5. External users (query 3)

What a split or a rename breaks: every symbol of another package that calls or references the crate, counted by distinct symbols used.

```cypher
MATCH (u:Symbol)-[:CALLS|REFERENCES]->(s:Symbol) WHERE s.package IN ['antares-model','antares-ql','antares-matcher','antares-residual','antares-jsonld','antares-store'] AND u.package<>s.package AND NOT u.is_test RETURN s.package, u.package, count(DISTINCT s) ORDER BY s.package, u.package;
```

```
35 rows, 78 ms
s.package | u.package | COUNT(DISTINCT s._ID)
antares-jsonld | antares-api | 69
antares-jsonld | antares-broker | 3
antares-jsonld | antares-matcher | 8
antares-jsonld | antares-notifier | 10
antares-jsonld | antares-ops | 11
antares-jsonld | antares-ql | 8
antares-jsonld | antares-store | 2
antares-jsonld | joinedcontext-policy | 6
antares-matcher | antares-api | 6
antares-model | antares-api | 53
antares-model | antares-broker | 4
antares-model | antares-bus | 7
antares-model | antares-icsr | 4
antares-model | antares-jsonld | 13
antares-model | antares-matcher | 1
antares-model | antares-mcp | 2
antares-model | antares-notifier | 5
antares-model | antares-ops | 4
antares-model | antares-plugin-example | 6
antares-model | antares-ql | 6
antares-model | antares-sql | 15
antares-model | antares-store | 7
antares-model | antares-ws | 2
antares-model | joinedcontext-policy | 4
antares-ql | antares-api | 53
antares-ql | antares-matcher | 18
antares-ql | antares-ops | 34
antares-ql | antares-plugin-example | 3
antares-ql | antares-sql | 51
antares-ql | antares-store | 3
antares-ql | joinedcontext-policy | 13
antares-store | antares-api | 138
antares-store | antares-broker | 33
antares-store | antares-plugin-example | 45
antares-store | antares-sql | 90
```

## 6. Public items no other crate uses (query 4)

The candidates for `pub(crate)` before a crate is published (T-3912 decides each; a type in a public signature stays `pub` whatever this says).

```cypher
MATCH (p:Package)-[:CONTAINS]->(:File)-[:DEFINES]->(s:Symbol) WHERE p.name IN ['antares-model','antares-ql','antares-matcher','antares-residual','antares-jsonld','antares-store'] AND s.visibility='pub' AND NOT s.is_test AND s.kind IN ['Function','Struct','Enum','Constant','TypeAlias','Trait','StaticMethod','Method'] AND NOT EXISTS { MATCH (u:Symbol)-[:CALLS|REFERENCES]->(s) WHERE u.package <> p.name } RETURN p.name, s.kind, s.path ORDER BY p.name, s.path;
```

77 rows: antares-jsonld 21, antares-matcher 1, antares-model 5, antares-ql 9, antares-residual 37 (no workspace user), antares-store 4.

```
antares-matcher | Function | antares_matcher::would_notify
antares-model | Struct | antares_model::IsoDuration
antares-model | Constant | antares_model::error::ERROR_TYPE_BASE
antares-model | Struct | antares_model::error::ProblemDetails
antares-model | Constant | antares_model::id::TenantId::RESERVED_EXACT
antares-model | Constant | antares_model::id::TenantId::RESERVED_PREFIXES
antares-ql | Struct | antares_ql::Link
antares-ql | Constant | antares_ql::MAX_NARROWED_Q_NODES
antares-ql | Constant | antares_ql::SYSTEM_ATTRS
antares-ql | TypeAlias | antares_ql::geo::Georel
antares-ql | Function | antares_ql::regex::cached
antares-ql | Function | antares_ql::regex::compiles
antares-ql | Function | antares_ql::regex::len
antares-ql | Function | antares_ql::regex::retained_bytes
antares-ql | Constant | antares_ql::scope::MAX_SCOPE_Q_BYTES
antares-jsonld | Method | antares_jsonld::context::Context::merge_core_object
antares-jsonld | Method | antares_jsonld::context::Context::outside
antares-jsonld | Method | antares_jsonld::context::Context::term
antares-jsonld | Method | antares_jsonld::context::Context::term_count
antares-jsonld | Method | antares_jsonld::context::Context::within
antares-jsonld | Method | antares_jsonld::context::Context::within_types
antares-jsonld | Constant | antares_jsonld::context::DEFAULT_VOCAB
antares-jsonld | Struct | antares_jsonld::context::TermDef
antares-jsonld | Constant | antares_jsonld::expand::ATTR_TYPES
antares-jsonld | Function | antares_jsonld::expand::expand_types
antares-jsonld | Function | antares_jsonld::expand::is_ngsi_null_langmap
antares-jsonld | TypeAlias | antares_jsonld::loader::CacheWriter
antares-jsonld | TypeAlias | antares_jsonld::loader::HookFuture
antares-jsonld | Method | antares_jsonld::loader::Loader::evict
antares-jsonld | Method | antares_jsonld::loader::Loader::resolve_quiet
antares-jsonld | StaticMethod | antares_jsonld::loader::Loader::with_client
antares-jsonld | StaticMethod | antares_jsonld::loader::Loader::with_policy
antares-jsonld | TypeAlias | antares_jsonld::loader::LocalLookup
antares-jsonld | Constant | antares_jsonld::loader::MAX_REDIRECTS
antares-jsonld | Struct | antares_jsonld::loader::PolicyResolver
antares-jsonld | TypeAlias | antares_jsonld::loader::UsageBump
antares-residual | Enum | antares_residual::Cmp
antares-residual | Enum | antares_residual::Error
antares-residual | Enum | antares_residual::Expr
antares-residual | Enum | antares_residual::Func
antares-residual | Method | antares_residual::Func::arity
antares-residual | Enum | antares_residual::Lit
antares-residual | Constant | antares_residual::MAX_DEPTH
antares-residual | Constant | antares_residual::MAX_NODES
antares-residual | Constant | antares_residual::MAX_STEPS
antares-residual | Constant | antares_residual::MAX_STRING_BYTES
antares-residual | Enum | antares_residual::Member
antares-residual | Enum | antares_residual::Path
antares-residual | StaticMethod | antares_residual::Path::value
antares-residual | Struct | antares_residual::Program
antares-residual | Method | antares_residual::Program::eval
antares-residual | Enum | antares_residual::Spatial
antares-residual | Enum | antares_residual::Truth
antares-residual | Method | antares_residual::Truth::and
antares-residual | Method | antares_residual::Truth::or
antares-residual | Function | antares_residual::compile
antares-residual | Function | antares_residual::eval
antares-residual | Constant | antares_residual::geometry::MAX_GEO_VERTICES
antares-residual | Constant | antares_residual::pattern::MAX_PATTERN_BYTES
antares-residual | Constant | antares_residual::pattern::MAX_PROGRAM_BYTES
antares-residual | Function | antares_residual::pattern::ecmascript
antares-residual | Function | antares_residual::pattern::like
antares-residual | Struct | antares_residual::time::Date
antares-residual | StaticMethod | antares_residual::time::Date::parse
antares-residual | Struct | antares_residual::time::DateTime
antares-residual | Constant | antares_residual::time::DateTime::MAX
antares-residual | Constant | antares_residual::time::DateTime::MIN
antares-residual | StaticMethod | antares_residual::time::DateTime::from_unix_micros
antares-residual | Method | antares_residual::time::DateTime::offset_minutes
antares-residual | StaticMethod | antares_residual::time::DateTime::parse
antares-residual | Method | antares_residual::time::DateTime::unix_micros
antares-residual | Struct | antares_residual::time::Time
antares-residual | StaticMethod | antares_residual::time::Time::parse
antares-store | StaticMethod | antares_store::StoreMode::names
antares-store | Method | antares_store::TemporalDriver::ping
antares-store | Trait | antares_store::TemporalDriverExt
antares-store | Function | antares_store::record_forward_via_mutate
```

## 7. After T-3911: the loader behind `fetch`

`antares-jsonld` splits in two. `src/pinned.rs` holds what needs no network: the pinned core context, `core_context()`, the size and URL-count limits, the `ContextLoader` trait, `PinnedOnly` (answers the pinned URLs and refuses every other with `LdContextNotAvailable`) and `resolve_with`, which resolves an `@context` value through any loader. `src/loader.rs` (the HTTP client, the `EgressPolicy`, the moka cache, `uuid` for cache keys) compiles only with the `fetch` feature, which turns on the optional `tokio`, `reqwest`, `rustls`, `moka`, `futures-util`, `uuid` and the wasm timer crates. `default` is empty. The broker side (`antares-api`, `antares-broker`, `antares-notifier`, `antares-wasm`, the `antares-ops` test kit) names `features = ["fetch"]`; `antares-ql` and `antares-matcher` use the core context only and drop to role `ngsild` with it.

`cargo tree -p antares-jsonld -e normal --no-default-features --prefix none`: 111 distinct crates at origin/dev `7d74f43` (reqwest, hyper, rustls, ring, tokio, moka, futures-util, uuid among them), 18 after. The workspace job checks `cargo check --locked --target wasm32-unknown-unknown -p antares-jsonld --no-default-features`; it failed before `uuid` moved under `fetch` (getrandom has no wasm32 source without a feature).

- Decided (worker-2, owner may override): an unknown context URL without `fetch` answers `LdContextNotAvailable`, the error the fetching loader already returns for a URL its policy refuses; the crate has no `LoadingDocumentFailed` variant and a new one would be a new public error for one case. Options: new variant, reuse (chosen).

## 8. The check

`dev/check-module-rules.py` runs M3 beside M1 and M2 in the workspace job's module-rules step (`.github/workflows/workspace.yml`) and in `dev/preflight.sh`. Its `--self-test` plants six M3 violations on a synthetic workspace whose clean `ngsild` crates include one with `tokio` behind a non-default `fetch` feature: a normal and a build dependency on `tokio`, a dependency on `antares-api` and on `antares-store`, `default` turning on `fetch`, and `default` naming the optional dependency itself. An unknown role was already a planted violation.

## Related

- `docs/adr/ADR-0027-surfaces-architecture.md` (roles), `docs/src/extending.md` (the module rules)
- T-3911, T-3912, T-3913
