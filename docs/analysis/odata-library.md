# OData 4.01 library for Rust: survey and crate design (T-3448, research half)

Sources: Antares-NGSI-LD-Context-Broker `7185e1a` (branch `agent/worker-2/T-3436`; only `deny.toml`,
`Cargo.toml` and `fuzz/Cargo.toml` were read). No other repository of ours was read. External
facts were read on 2026-10-10 from the crates.io API (`https://crates.io/api/v1/crates?q=...` and
`/api/v1/crates/<name>`), the GitHub REST API (`/repos/<owner>/<repo>`, `/search/repositories`,
`/commits/<branch>`, `/git/trees/<sha>?recursive=1`), shallow clones of each GitHub project (grep only,
nothing built or run), `static.crates.io` crate tarballs (grep only), Maven Central, NuGet, PyPI and
the npm registry, and the OASIS OData 4.01 HTML specifications on `docs.oasis-open.org`.
lib.rs answered HTTP 403 to both WebFetch and curl, so lib.rs was not searched.
No code relation inside our repositories is mapped in this document, so no `cg` query was needed.
The ADR (`docs/adr/ADR-00NN-odata-library.md`) is written in a later step and is not part of this file.

## 1. Survey of Rust OData crates and projects

### 1.1 Method

- crates.io search `q=odata` returned 80 crates (`meta.total`), `q=csdl` 14, `q=edm` 81,
  `q=olingo` 3. Every crate whose description names OData, CSDL or EDMX was opened with
  `/api/v1/crates/<name>`; crates that only name a vendor API (Dynamics 365, SharePoint, Microsoft
  Graph, Dataverse, Power BI, Copernicus, RESO) as an HTTP client are listed in 1.4 without a row.
- GitHub search `odata language:Rust` returned 32 repositories, `csdl language:Rust` 2 (neither
  about OData), `odata v4 rust` 2, `edmx language:Rust` 1 (not about OData). Each OData repository
  was shallow-cloned and grepped.
- Coverage columns come from the project README or module docs plus a grep of `*.rs` for
  `$filter`, `$expand`, `$select`, `$orderby`, `$search`, `$compute`, `$apply`, `$batch|multipart`,
  `EntityContainer`, `edmx`, `$metadata`, `odata.context`. "partial" means the README or a coverage
  file says so or only some options are present. A "-" means no hit and no README claim.
- `unsafe`: `grep -E '\bunsafe\b'` over the project's `*.rs` (comment lines dropped), and whether
  the crate root or workspace lints carry `forbid(unsafe_code)`. "not checked" where the source
  was not cloned.
- "Issues" is GitHub `open_issues_count`, which counts open issues plus open pull requests.

### 1.2 Published crates (crates.io)

| Crate | Latest | Last release | Licence (crates.io) | OData version | Query options parser | Resource path | CSDL XML | CSDL JSON | JSON format writer | `$batch` multipart | `$batch` JSON | `unsafe` | Downloads | Issues (repo) | Maintenance | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `odata-params` (JenChampagne/odata-params) | 0.4.0 | 2024-07-30 | MIT OR Apache-2.0 | v4 (README) | `$filter` only (README "provides a parser for OData v4 `$filter` expressions"; lambdas, aliases, `has` not implemented) | - | - | - | - | - | - | 0 hits | 182484 | 1 | last push 2024-10-24 | no |
| `cf-gears-toolkit-odata` (constructorfabric/gears-rust `libs/toolkit-odata`) | 0.9.7 | 2026-09-20 | Apache-2.0 | v4 subset | `$filter` (peg grammar, includes code from `odata-params` per its README), `$orderby`, signed cursor paging, length limits (`src/limits.rs`) | - | - | - | - (own `Page<T>` type, not OData JSON) | - | - | workspace lints `unsafe_code = "forbid"` (`Cargo.toml:200`) | 28048 | 621 (whole monorepo) | active (push 2026-10-10) | no: tied to `toolkit-gts`, `toolkit-canonical-errors` |
| `cf-gears-toolkit-odata-macros` | 0.6.13 | 2026-09-05 | Apache-2.0 | v4 subset | derive macros for the crate above | - | - | - | - | - | - | as above | 2398 | as above | active | no |
| `cf-modkit-odata` (cyberfabric/cyberfabric-core) | 0.8.0 | 2026-05-06 | Apache-2.0 | v4 subset | predecessor name of the gears crate (same description family) | unknown | unknown | unknown | unknown | unknown | unknown | not checked (repository URL answers 404) | 8668 | unknown | repository gone | no |
| `datafusion-odata` (kamu-data/datafusion-odata) | 55.0.0 | 2026-09-28 | Apache-2.0 | EDMX 1.0 / `DataServiceVersion` 1.0..3.0 (`src/metadata.rs:1-2`), Atom payloads (`src/atom.rs`) | `$filter` (via `odata-params`), `$select`, `$orderby` | collection handlers | writer, v3 EDMX | - | - (Atom XML) | - | - | 0 hits | 50079 | 0 | active, follows DataFusion releases | no: OData v3 shapes, DataFusion-bound |
| `odatav4-parser` (knackstedt/rust-odatav4) | 0.0.3 | 2025-12-21 | MIT | v4 (README links 4.01 URL conventions) | query string to AST and SQL; `$filter` incl. `any`/`all`/`in`, `$select`, `$expand`, `$orderby`, `$top`, `$skip` (README) | - | - | - | - | - | - | 0 hits | 83 | 0 | 3 versions in 2 days, no push since 2025-12-21 | no |
| `odata-parser-rs` (Datavirke/odata-parser-rs) | 0.1.2 | 2021-11-13 | MIT | OData 3.0 / EDMX 1.0 (README) | - | - | reader only, "absolute bare-minimum" (README) | - | - | - | - | 0 hits | 4988 | 0 | inactive since 2021 | no |
| `sap-odata-core` (kts982/sap-odata-explorer) | 0.2.0 | 2026-10-01 | MIT | V2 and V4 client (README) | client-side query building | client | reader (roxmltree), SAP annotations | - | - | client hit for `multipart` | - | `unsafe` in `crates/core/src/sspi.rs:25-39` (Windows SSPI FFI) | 82 | 0 | active | no: SAP client explorer |
| `nv-redfish-csdl-compiler` (dsx-ai-factory/nv-redfish) | 0.19.0 | 2026-10-09 | Apache-2.0 | CSDL 4.0 (`Edmx Version="4.0"` in `csdl-compiler/src/lib.rs:147`) | - | - | reader (quick-xml serde) for Redfish schemas, Rust code generator | - | - | - | - | generated code emits `unsafe impl Send/Sync` (`csdl-compiler/src/generator/rust/struct_def.rs:135-137`) | 167809 | 5 | active | no: build-time Redfish codegen |
| `parse-sap-odata` (ChrisWhealy/parse-sap-odata) | 1.6.2 | 2026-04-13 | MIT | SAP V2 (description) | - | - | V2 metadata to Rust codegen | - | - | - | - | not checked | 49910 | 0 | push 2026-04-13 | no: V2 only |
| `odata-rust-generator` (Datavirke) | 0.6.0 | 2021-11-14 | MIT | OData 3.0 (via `odata-parser-rs`) | - | - | codegen | - | - | - | - | not checked | 11354 | 0 | inactive since 2021 | no |
| `odata-simple-client` (Datavirke) | 0.2.6 | 2023-01-14 | MIT | client | client | client | - | - | - | - | - | not checked | 7716 | 0 | inactive since 2023 | no |
| `odata_client`, `odata_client_codegen`, `odata_client_derive`, `odata_client_util` (gitlab tobz1000/odata_client_rs) | 0.1.0 | 2021-01-24 | Apache-2.0 | client codegen | client | client | codegen input | - | - | - | - | not checked | 1675 (`odata_client`) | unknown (GitLab not queried) | one release, 2021 | no |
| `reso-client` (jeremeybingham/reso_client) | 0.2.1 | 2025-11-07 | MIT | 4.0 client (RESO Web API) | client | client | - | - | - | - | - | not checked | 52 | 1 | push 2026-03-30 | no |

Cypher: none (no code relation in our repositories).

### 1.3 Unpublished Rust OData projects (GitHub)

| Project | Version in Cargo.toml | Last commit | Licence | OData version | Query options parser | Resource path | CSDL XML | CSDL JSON | JSON format writer | `$batch` multipart | `$batch` JSON | `unsafe` | Stars / Issues | Maintenance | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| xtofs/odata-rs (Christof Sprenger) | 0.0.1 | 2026-06-18 (37 commits) | MIT (`LICENSE`; `Cargo.toml` says `MIT OR Apache-2.0`) | 4.0 fixtures (`crates/csdl-edm/data/inputs/example89.csdl.xml:4`) | partial, per `crates/url/docs/abnf-coverage.md`: `$select`, `$filter` (`commonExpr` subset), `$expand`, `$orderby` partial; `$top`, `$skip`, `$count` done; lambdas, `$search`, `$compute`, key predicates deferred | partial (`ResourcePath`) | reader and writer (quick-xml 0.40) | reader and writer (serde_json) | partial (`@odata.context` in `crates/service/src/builder.rs:613`) | - | - | 0 hits; no `forbid` attribute | 0 / 0 | early stage (README) | no; best design reference (crate split `csdl-edm`, `odata-rs-url`, `odata-rs-service`) and a source of CSDL XML/JSON fixture pairs |
| adiepenbrock/odata-rs (`odata-core`, `odata-axum`) | 0.1.0 | 2025-06-22 | MIT (`LICENSE`) | 4.01 (claim in `odata-core/src/lib.rs:3`) | `$filter`, `$expand`, `$select`, `$orderby`, `$top`, `$skip`, `$count`, `$search`, `$format` (module docs) | - | - | - | - | - | - | 0 hits | 1 / 0 | no push since 2025-06 | no |
| mibes404/odata-rs | 0.1.0 | 2026-07-22 | MIT | 4 (README) | `$filter`, `$orderby` hits | partial | reader (`odata-edm`, quick-xml serde) | - | partial (`odata.context` hits) | - | - | 0 hits | 2 / 0 | "very early stage ... not ready for general use" (README) | no |
| danbluhmhansen/odata | 0.1.0 | 2025-08-30 | LGPL-3.0 (`Cargo.toml`), GPL-3.0/LGPL-3.0 (README) | 4.01 (doc link `src/lib.rs:106`) | system query options (chumsky + logos) | - | - | - | - | - | - | `#![forbid(unsafe_code)]` in `src/lib.rs` | 0 / 0 | no push since 2025-08 | no: LGPL-3.0 is not on our allow list |
| balena-io-experimental/odata-parser-rs | 0.1.0 | 2019-09-10 | none found (no `LICENSE`, no `license` key) | 4.01 (description) | yes (nom) | yes | - | - | - | `$batch` keyword hit | - | 0 hits | 3 / 0 | inactive since 2019 | no |
| hasura/ndc-odata | (none) | 2024-01-19 | none found | 4.01 (README) | client side | client | reader (quick-xml) | JSON metadata config | - | - | - | 0 hits | 1 / 1 | inactive since 2024 | no: connector, client side |
| eijiensing/odatax | 0.1.0 | 2026-07-31 | none found | client | client | - | reader for codegen | - | - | - | - | 0 hits | 0 / 0 | early | no |
| peterwest-1/flodata | 0.1.0 | 2026-05-02 | MIT | v4 client | query builder (client) | - | - | - | - | - | - | 0 hits | 0 / 0 | early | no |
| mmgeorge/rust-odata | (not read) | 2018-02-15 | MIT/Apache text under `docs/` only | 4 (README) | - | macro-declared entity sets | `$metadata` hits | - | - | - | - | `unsafe impl Sync/Send for Model` (`src/model.rs:26-27`) | 10 / 5 | inactive since 2018 | no |
| zkud/cim | 0.3.6 (crate `cim`, 2022-09-03) | 2023-03-20 | MIT | V2/V4 metadata (description) | - | - | reader (xml-rs), CDS codegen | - | - | - | - | not checked | 2 / 4 | inactive | no |
| cwoodruff/odata-rust-modular-monolith | 1.0.0 | 2026-09-21 | MIT | unknown | - (0 hits on every keyword) | - | - | - | - | - | - | 0 hits | 0 / 0 | - | no: no OData code found |

Cypher: none.

### 1.4 Crates seen and dropped without a row

Vendor clients or tools that only call a specific OData service: `d365-odata-mcp`, `sap-odata-cli`,
`parse-sap-atom-feed`, `odata-generator` (generates Java), `folketinget-api-models`, `sharepoint`,
`graph-rs-sdk`, `powerplatform-dataverse-service-client`, `drasi-source-dataverse`,
`drasi-bootstrap-dataverse`, `fm-script-client`, `gegevensmagazijn`, `pbirs`, `mcp-erp`,
`omnia-azure-table`, `copernicus_explorer`, `faucet-source-rest`. GitHub-only clients of the same
kind: consetto/sap-cloud-alm-odata-mcp, FlintsLabs/d365-odata-mcp, Datavirke/odata-simple-client,
kelko/rodata (archived), suhrawardi/odata_playground (2020), lighthouse-no forks of parse-sap-odata.

## 2. Overall verdict: build

Build our own crates. Reasons, each from the tables in section 1:

1. No crate or project covers CSDL JSON, the OData JSON format writer and `$batch` together; none
   covers `$batch` in either form at all. Only xtofs/odata-rs has CSDL XML and CSDL JSON, and it is
   0.0.1, unpublished, and defers key predicates, lambdas, `$search` and `$compute`.
2. The published query parsers are `$filter`-centred (`odata-params`, `cf-gears-toolkit-odata`) or
   very young (`odatav4-parser` 0.0.3, 83 downloads). The one with input limits
   (`cf-gears-toolkit-odata`) depends on its own framework crates (`toolkit-gts`,
   `toolkit-canonical-errors`), which breaks the "protocol libraries know nothing about the host"
   rule in `tasks/GOAL-antares-surfaces.md` point 6 from the other side: we would link a foreign
   framework.
3. The server-side projects target OData v3 shapes (`datafusion-odata`: EDMX 1.0, Atom) or are
   clients and code generators.
4. The one parser with `forbid(unsafe_code)` and a broad grammar (danbluhmhansen/odata) is
   LGPL-3.0, which `deny.toml` does not allow.
5. Forking xtofs/odata-rs would save the CSDL reader/writer skeleton only; its URL crate sits on the
   `url` crate, has no depth/node/length limits in its coverage table, and its service crate binds
   axum and sqlx. Building from the OASIS ABNF with explicit limits is less work than retrofitting
   limits and `forbid(unsafe_code)` into it. Its MIT licence allows reading and citing its
   fixtures; copying code would need its notice.

Fork candidates kept for reference, not dependency: xtofs/odata-rs (design and fixtures),
`cf-gears-toolkit-odata` (`src/limits.rs` shows which limits real users needed).

Where the crates live (task step 2). Decided (worker-2, owner may override): inside the Antares
workspace as `crates/odata-*` with `publish = false` first, a CI check that no `odata-*` crate
depends on an `antares-*` crate (T-3447 module rules), and extraction to `marek-mraz-jc/odata-rs`
with `git subtree split` once the API is stable. Reason: one CI, one `deny.toml`, one fuzz
workspace (`fuzz/`, excluded at `Cargo.toml:21`), one supply-chain gate, while the dependency rule
keeps the extraction a mechanical step. Options were: inside the Antares workspace, or its own repository
`marek-mraz-jc/odata-rs` from the start.

## 3. Crate design (no `antares-*` dependency)

Dependency direction (arrows read "depends on"); every crate `#![forbid(unsafe_code)]`, matching the
workspace lint `unsafe_code = "forbid"` at `Cargo.toml:156`:

```
odata-csdl   (serde_json, quick-xml)
odata-url    (no OData crate; optional feature `edm` -> odata-csdl for model resolution)
odata-json   (serde_json; odata-csdl, odata-url for context URLs)
odata-batch  (serde_json, memchr; no OData crate)
odata-axum   (optional; axum, http; all four above)
```

Every limit is a field of a `Limits` struct passed by the caller; there is no hidden default
on the request path. `Limits::strict()` gives conservative values for tests and docs, chosen in
the ADR. All counters use checked arithmetic and return `LimitExceeded` instead of wrapping.

### 3.1 `odata-url`: resource path and system query options to a typed AST

```rust
pub struct Limits {
    pub max_path_len: usize,          // bytes of the percent-decoded resource path
    pub max_query_len: usize,         // bytes of the raw query string
    pub max_path_segments: usize,
    pub max_query_options: usize,     // count of `name=value` pairs
    pub max_expr_depth: usize,        // nesting of commonExpr (parens, unary, lambdas)
    pub max_expr_nodes: usize,        // AST nodes over the whole request
    pub max_expand_depth: usize,      // nested $expand levels; `$levels=max` is clamped to this
    pub max_expand_items: usize,
    pub max_select_items: usize,
    pub max_orderby_items: usize,
    pub max_in_list_len: usize,       // items in `x in (...)`
    pub max_literal_len: usize,       // one string/binary/geo literal
    pub max_search_terms: usize,
    pub max_top: Option<u64>,         // None: no cap at parse time (paging applies later)
}

pub struct Request<'a> { pub method: Method, pub path: &'a str, pub query: &'a str }   // own enum, no `http` dependency

pub enum Method { Get, Post, Put, Patch, Delete }

/// Syntax only: identifiers stay unresolved strings.
pub fn parse(req: &Request<'_>, limits: &Limits) -> Result<ODataUri, UrlError>;

/// Feature `edm`: resolves identifiers, key predicates, casts and bound operations
/// against the model; type-checks $filter/$orderby/$compute.
#[cfg(feature = "edm")]
pub fn resolve(uri: ODataUri, model: &odata_csdl::Model, limits: &Limits)
    -> Result<ResolvedUri, UrlError>;

/// Canonical form (used by proptest AST -> URL -> AST and for @odata.nextLink).
pub fn to_string(uri: &ODataUri) -> String;

pub enum ODataUri {
    ServiceDocument,
    Metadata { options: MetadataOptions },
    Batch,
    Entity { id: String, options: QueryOptions },           // $entity?$id=
    Resource { path: ResourcePath, options: QueryOptions },
}

pub struct ResourcePath { pub segments: Vec<Segment> }
pub enum Segment {
    Identifier(String),            // entity set, singleton, navigation, property, operation import
    Key(KeyPredicate),             // (1) or (a=1,b='x') or /key-as-segment
    Cast(QualifiedName),
    Count, Ref, Value, Each, All,  // $count $ref $value $each $all
    Filter(Box<Expr>),             // /$filter(...) path segment
    Call { name: QualifiedName, args: Vec<(String, Expr)> },
}

pub struct QueryOptions {
    pub filter: Option<Expr>, pub select: Option<Vec<SelectItem>>, pub expand: Option<Vec<ExpandItem>>,
    pub orderby: Option<Vec<OrderBy>>, pub top: Option<u64>, pub skip: Option<u64>,
    pub count: Option<bool>, pub search: Option<SearchExpr>, pub compute: Option<Vec<Compute>>,
    pub format: Option<Format>, pub skiptoken: Option<String>, pub deltatoken: Option<String>,
    pub schemaversion: Option<String>, pub index: Option<i64>, pub id: Option<String>,
    pub apply: Option<RawApply>,   // kept raw; any use is UrlError::NotImplemented unless subset adds it
    pub aliases: Vec<(String, Expr)>, pub custom: Vec<(String, String)>,
}

pub enum Expr { Literal(Literal), Path(Vec<String>), Alias(String), Root(Vec<String>),
    Unary(UnaryOp, Box<Expr>), Binary(Box<Expr>, BinaryOp, Box<Expr>),
    In(Box<Expr>, Vec<Expr>), Call(Function, Vec<Expr>),
    Lambda { path: Vec<String>, kind: LambdaKind, var: Option<String>, body: Option<Box<Expr>> },
    Cast(Option<Box<Expr>>, QualifiedName), IsOf(Option<Box<Expr>>, QualifiedName) }

pub struct UrlError { pub kind: UrlErrorKind, pub offset: usize }
pub enum UrlErrorKind {
    Syntax { expected: &'static str },
    LimitExceeded { limit: LimitKind, max: u64 },
    DuplicateOption(&'static str),
    NotImplemented(Feature),   // maps to HTTP 501 with the OData error format
    Unresolved(String),        // feature `edm`
    Type(String),              // feature `edm`
}
```

Rules: names of system query options are matched case-insensitively with or without `$` (Protocol
4.01 section 11.2.1, quoted in 3.6); dollar-prefixed path segments are case-sensitive (ABNF comment
at `odata-abnf-construction-rules.txt:87`, "dollar-prefixed path segments are case-sensitive!").
Fuzz target: `parse` on arbitrary bytes split into path and query. Proptest: `ODataUri` generator
-> `to_string` -> `parse` equals input. Data-driven test: the OASIS test cases (section 4.1).

### 3.2 `odata-csdl`: EDM model, CSDL XML and CSDL JSON, validation

```rust
pub struct Model { pub version: CsdlVersion, pub references: Vec<Reference>, pub schemas: Vec<Schema> }
pub enum CsdlVersion { V4_0, V4_01 }
pub struct Schema { pub namespace: String, pub alias: Option<String>,
    pub entity_types: Vec<EntityType>, pub complex_types: Vec<ComplexType>, pub enum_types: Vec<EnumType>,
    pub type_definitions: Vec<TypeDefinition>, pub actions: Vec<Action>, pub functions: Vec<Function>,
    pub terms: Vec<Term>, pub entity_container: Option<EntityContainer>, pub annotations: Vec<Annotations> }
pub struct EntityType { pub name: String, pub key: Vec<PropertyRef>, pub base_type: Option<QualifiedName>,
    pub open: bool, pub has_stream: bool, pub properties: Vec<Property>,
    pub navigation_properties: Vec<NavigationProperty>, pub annotations: Vec<Annotation> }
pub struct Property { pub name: String, pub ty: TypeRef, pub nullable: bool, pub max_length: Option<MaxLength>,
    pub precision: Option<u32>, pub scale: Option<Scale>, pub srid: Option<Srid>, pub annotations: Vec<Annotation> }
pub struct EntityContainer { pub name: String, pub entity_sets: Vec<EntitySet>, pub singletons: Vec<Singleton>,
    pub action_imports: Vec<ActionImport>, pub function_imports: Vec<FunctionImport> }
// NavigationProperty, EntitySet (with NavigationPropertyBinding), Annotation (Term + Expression), etc.

pub struct ModelBuilder { /* ... */ }
impl ModelBuilder { pub fn new(version: CsdlVersion) -> Self; pub fn schema(&mut self, s: Schema) -> &mut Self;
    pub fn build(self) -> Result<Model, Vec<ValidationError>>; }

pub fn validate(model: &Model) -> Result<(), Vec<ValidationError>>;   // names, keys, references, partners, referential constraints (T-3761), bindings
pub fn write_xml<W: std::io::Write>(model: &Model, w: W) -> std::io::Result<()>;
pub fn write_json<W: std::io::Write>(model: &Model, w: W) -> std::io::Result<()>;   // V4_01 only
pub fn metadata_etag(model: &Model) -> String;   // stable hash of the XML form, for odata.metadataEtag

pub struct ReadLimits { pub max_bytes: usize, pub max_depth: usize, pub max_elements: usize,
    pub max_attr_len: usize, pub max_schemas: usize }
#[cfg(feature = "read")] pub fn read_xml(input: &[u8], limits: &ReadLimits) -> Result<Model, CsdlError>;
#[cfg(feature = "read")] pub fn read_json(input: &[u8], limits: &ReadLimits) -> Result<Model, CsdlError>;
```

The readers exist for round-trip tests (model -> XML -> model, model -> JSON -> model), for reading
OASIS vocabularies, and for a fuzz target; the server only writes. XML is written with
`quick-xml` (forbids unsafe, section 6). CSDL JSON is 4.01 only: the JSON Schema says "OData Common
Schema Definition Language (CSDL) JSON Representation Version 4.01" (`schemas/csdl.schema.json`,
section 4.2).

### 3.3 `odata-json`: JSON format, control information, ETags, errors

```rust
pub enum MetadataLevel { None, Minimal, Full }            // JSON Format 4.01 section 3.1
pub struct Ctx<'a> { pub service_root: &'a str, pub level: MetadataLevel, pub version: ODataVersion }
pub enum ODataVersion { V4_0, V4_01 }                     // controls `@odata.` vs `@` prefix

pub fn context_url(ctx: &Ctx<'_>, target: &ContextTarget<'_>) -> String;   // section 4.5.1
pub fn write_entity<W: std::io::Write>(ctx: &Ctx<'_>, e: &EntityValue, w: W) -> std::io::Result<()>;
pub fn write_collection<W: std::io::Write>(ctx: &Ctx<'_>, items: &mut dyn Iterator<Item = EntityValue>,
    count: Option<u64>, next_link: Option<&str>, w: W) -> std::io::Result<()>;
pub fn write_entity_reference<W: std::io::Write>(ctx: &Ctx<'_>, ids: &[&str], w: W) -> std::io::Result<()>;
pub fn write_service_document<W: std::io::Write>(ctx: &Ctx<'_>, c: &odata_csdl::EntityContainer, w: W) -> std::io::Result<()>;

pub struct BodyLimits { pub max_bytes: usize, pub max_depth: usize, pub max_properties: usize,
    pub max_nested_entities: usize, pub max_bind_links: usize, pub max_collection_len: usize }
pub fn read_entity_body(input: &[u8], ty: &odata_csdl::EntityType, model: &odata_csdl::Model,
    limits: &BodyLimits) -> Result<EntityPayload, BodyError>;    // includes @odata.bind and deep insert

pub struct ETag { pub weak: bool, pub value: String }
pub fn parse_if_match(header: &str, max_tags: usize) -> Result<IfMatch, HeaderError>;   // `*` or list
pub struct Prefer { pub return_: Option<ReturnPref>, pub maxpagesize: Option<u64>,
    pub include_annotations: Option<String>, pub respond_async: bool }
pub fn parse_prefer(header: &str, max_len: usize) -> Result<Prefer, HeaderError>;
pub fn negotiate_version(odata_version: Option<&str>, max_version: Option<&str>) -> Result<ODataVersion, HeaderError>;

pub struct ODataError { pub code: String, pub message: String, pub target: Option<String>,
    pub details: Vec<ErrorDetail> }                         // section 21.1; no innererror by default
pub fn write_error<W: std::io::Write>(e: &ODataError, w: W) -> std::io::Result<()>;
```

`innererror` is left out of the type on purpose: it is where servers leak stack traces. Error
messages never echo literal values from the request (a hidden attribute's value must not come back
through an error text).

### 3.4 `odata-batch`: multipart and JSON batch

```rust
pub struct Limits { pub max_body_bytes: usize, pub max_requests: usize, pub max_changesets: usize,
    pub max_changeset_requests: usize, pub max_part_header_bytes: usize, pub max_headers_per_part: usize,
    pub max_depends_on: usize, pub max_id_len: usize, pub max_url_len: usize }

pub struct Batch { pub items: Vec<BatchItem> }
pub enum BatchItem { Request(SubRequest), ChangeSet { atomicity_group: Option<String>, requests: Vec<SubRequest> } }
pub struct SubRequest { pub id: Option<String>, pub method: Method, pub url: String,
    pub headers: Vec<(String, String)>, pub body: Vec<u8>, pub depends_on: Vec<String> }

pub fn boundary_from_content_type(content_type: &str) -> Result<String, BatchError>;
pub fn parse_multipart(body: &[u8], boundary: &str, limits: &Limits) -> Result<Batch, BatchError>;   // Protocol 11.7.7
pub fn parse_json(body: &[u8], limits: &Limits) -> Result<Batch, BatchError>;                      // JSON Format 19.1
/// Topological order of `depends_on` and `$<id>` references; rejects cycles,
/// unknown ids and forward references (Protocol 11.7.2, 11.7.3).
pub fn execution_order(batch: &Batch) -> Result<Vec<usize>, BatchError>;
pub fn write_multipart_response<W: std::io::Write>(r: &[SubResponse], boundary: &str, w: W) -> std::io::Result<()>;
pub fn write_json_response<W: std::io::Write>(r: &[SubResponse], w: W) -> std::io::Result<()>;
```

The multipart parser is our own, over a byte slice already bounded by `max_body_bytes`, using
`memchr` for the boundary search. `multer` was considered: it forbids unsafe and is MIT, but it is
stream-based and pulls `tokio`, `futures-util`, `spin` and `encoding_rs` (section 6), which a
pure, Miri-testable parser does not need. Protocol 11.7.7.1 says "The contents of a body part representing a change set MUST itself be a
multipart document (see [RFC2046]) with one body part for each operation in the change set", so the
parser accepts exactly two multipart levels and rejects a third. The same clause says "Services MUST
support all three formats for URLs of individual requests" (absolute URI, absolute path with `Host`,
relative path): the parser returns the URL as written and the caller checks that an absolute URI or
`Host` names this service before anything runs, so a batch cannot address another host. Fuzz targets: `parse_multipart`, `parse_json`.

### 3.5 `odata-axum` (optional, feature `axum` of the surface crate)

```rust
pub struct ODataState { pub model: std::sync::Arc<odata_csdl::Model>, pub url_limits: odata_url::Limits,
    pub body_limits: odata_json::BodyLimits, pub batch_limits: odata_batch::Limits }
pub struct ODataRequest(pub odata_url::ODataUri, pub odata_json::Ctx<'static>);   // FromRequestParts
pub struct ODataResponse { /* status, headers incl. OData-Version, body */ }       // IntoResponse
pub struct ODataErrorResponse { pub status: http::StatusCode, pub error: odata_json::ODataError } // IntoResponse
pub fn metadata_router(state: ODataState) -> axum::Router;   // GET $metadata (XML; JSON when 4.01 and asked)
```

The adapter only translates HTTP to the library types and back. It never sees an Antares type; the
Antares OData surface crate (T-3441) maps `ResolvedUri` to TS 104 175 operations and the ADR-0020
policy seam.

### 3.6 Exact subset per crate: filled from T-3441

Decided in ADR-0024 point 5 from the mapped rows of `odata-linkml-mapping.md` (T-3441); the list below is what there was to choose from. Everything outside
the chosen subset returns 501 Not Implemented with the OData error body (`UrlErrorKind::NotImplemented`).
What exists to choose from:

System query options, from the ABNF rule `systemQueryOption` (`abnf/odata-abnf-construction-rules.txt:276-290`
at the pinned commit in 4.1): `$compute`, `$deltatoken`, `$expand`, `$filter`, `$format`, `$id`,
`$count` (rule `inlinecount`), `$orderby`, `$schemaversion`, `$search`, `$select`, `$skip`,
`$skiptoken`, `$top`, `$index`; expand-item option `$levels` (line 323); `$apply` is defined in
`abnf/odata-aggregation-abnf.txt:82` (OData Data Aggregation extension), not in the core rule.
Protocol 4.01 section 11.2.1 (read from
`https://docs.oasis-open.org/odata/odata/v4.01/os/part1-protocol/odata-v4.01-os-part1-protocol.html`):
"System query options are prefixed with the dollar ($) character, which is optional in OData 4.01.
4.01 services MUST support case-insensitive system query option names specified with or without the
$ prefix." and evaluation order "$schemaversion ... $apply ... $compute $search $filter $count $orderby
$skip $top ... After applying any server-driven paging: $expand $select $format".

Top-level URL forms (ABNF `odataRelativeUri`, lines 88-92): `$batch`, `$entity?$id=`,
`$entity/<cast>?`, `$metadata`, resource path with query options.

Write and invoke forms (Protocol 4.01 section headings, same document):
- 11.4.2 Create an Entity; 11.4.2.1 link to related entities on create (`@odata.bind`);
  11.4.2.2 create related entities on create (deep insert)
- 11.4.3 Update an Entity (PATCH, PUT); 11.4.3.1 update related entities (deep update)
- 11.4.4 Upsert an Entity
- 11.4.5 Delete an Entity
- 11.4.6 Modifying relationships: 11.4.6.1 add a reference (`POST .../$ref`), 11.4.6.2 remove a
  reference, 11.4.6.3 change a single-valued reference, 11.4.6.4 replace all references
- 11.4.7 media entities (create, update stream, delete); 11.4.8 stream properties
- 11.4.9 properties directly: primitive, set to null, complex, collection
- 11.4.10 members of an ordered collection; 11.4.11 positional inserts
- 11.4.12 update a collection of entities (delta payload); 11.4.13 update members of a collection
  (`/$each`); 11.4.14 delete members of a collection
- 11.5.4 functions, 11.5.5 actions (bound and unbound); 11.5.2 action on members of a collection
- 11.6 asynchronous requests (`Prefer: respond-async`)
- 11.7 batch: 11.7.7 multipart format; JSON batch in JSON Format 4.01 section 19
- ETags for update conflicts: 11.4.1.1

## 4. Conformance sources (pinned)

SHAs are the default-branch heads read on 2026-10-10 with
`GET https://api.github.com/repos/oasis-tcs/<repo>/commits/main`; file paths were checked against
`GET /repos/oasis-tcs/<repo>/git/trees/<sha>?recursive=1` (tree not truncated) and the files
fetched from `raw.githubusercontent.com/oasis-tcs/<repo>/<sha>/<path>`.

| Repository | Default branch | Pinned commit (committer date, message) | Files verified at that commit | Licence |
|---|---|---|---|---|
| https://github.com/oasis-tcs/odata-abnf | `main` | `a31c8db229a0d8e957bee86512b01efe7ce74173` (2026-08-26, "Bump brace-expansion (override) (#151)") | `abnf/odata-abnf-construction-rules.txt` (1298 lines, header "OData ABNF Construction Rules Version 4.01 and 4.0"), `abnf/odata-abnf-testcases.yaml` (3770 lines, 840 `- Name:` test cases, 79 with `FailAt:`, a `Constraints:` block naming the identifiers the cases assume), `abnf/odata-aggregation-abnf.txt`, `abnf/odata-aggregation-testcases.yaml`, `abnf/odata-temporal-abnf.txt`, `abnf/odata-temporal-testcases.yaml`, `lib/testsuite.schema.json`, `lib/checkAbnf.js` | `LICENSE.md`: OASIS IPR Policy, "RF on RAND Terms Mode"; GitHub SPDX `NOASSERTION` |
| https://github.com/oasis-tcs/odata-csdl-schemas | `main` | `e6b66a96ec5f74edb0c0e192978a39f146f70d76` (2026-09-13, "Bump js-yaml from 4.3.1 to 4.3.2 (#123)") | `schemas/edmx.xsd` (root, imports `edm.xsd` with `schemaLocation="edm.xsd"` at line 66), `schemas/edm.xsd`, `schemas/csdl.schema.json` (JSON Schema draft-07, "CSDL JSON Representation Version 4.01"), `catalog.xml`, examples `examples/csdl-16.1.xml`/`.json`, `examples/csdl-16.2.xml`/`.json`, `examples/miscellaneous*.xml`/`.json`, `examples/counterexamples/` | `LICENSE.md`: OASIS IPR Policy, "RF on RAND Terms Mode"; SPDX `NOASSERTION` |
| https://github.com/oasis-tcs/odata-vocabularies | `main` | `3e36ad2a4930a47feaf7a7cd0b78ed4f2bafcfb2` (2026-09-13, "Bump js-yaml from 4.3.1 to 4.3.2 (#342)") | `vocabularies/Org.OData.Core.V1.xml`/`.json`, `vocabularies/Org.OData.Capabilities.V1.xml`/`.json`, `vocabularies/Org.OData.Measures.V1.xml`/`.json`; Measures includes `Org.OData.Core.V1` and `Org.OData.Validation.V1` (`edmx:Include` lines), so `vocabularies/Org.OData.Validation.V1.xml`/`.json` is needed too | `LICENSE.md`: OASIS IPR Policy, "RF on RAND Mode"; SPDX `NOASSERTION` |

How each source becomes a test (untested proposals, for the test plan T-3444):

1. `odata-url` data-driven test: read `odata-abnf-testcases.yaml`, run every case whose `Rule` is
   an entry point we implement (`odataUri`, `resourcePath`, `queryOptions`, single option rules,
   `commonExpr`), expect accept, or reject at the `FailAt` offset; build a test model from the
   `Constraints:` block so identifiers resolve. Cases for rules outside the subset must return
   `NotImplemented`, never accept. The case count per rule goes into the test report so coverage
   is visible.
2. Differential fuzzing: the repository generates a reference parser with `apg-js`
   (`package.json` script `generate-grammar`); a CI job can feed the `odata-url` fuzz corpus to it
   and compare accept/reject.
3. Every `$metadata` we emit: CSDL XML validated against `schemas/edmx.xsd` with the
   `catalog.xml` mapping (for example `xmllint --catalogs`), CSDL JSON against
   `schemas/csdl.schema.json`. The JSON Schema uses `\p{L}` style Unicode property patterns, so the
   validator must support them; `ajv` is what the repository itself uses (`devDependencies`), a Rust
   validator is unknown here and needs a check.
4. Cross-check XML against JSON: the repository's npm package `odata-csdl` (version 0.11.2 in its
   `package.json`, binary `odata-csdl-xml2json`) converts our CSDL XML to CSDL JSON; the result must
   equal our own `write_json` output.
5. Vocabulary terms we annotate with are read from the pinned vocabulary files (feature `read` of
   `odata-csdl`), so a misspelled term fails a test.

Safest default for using them: fetch at the pinned SHA in CI with a checksum, do not vendor the
files into the repository (their licence is the OASIS IPR policy, not an SPDX licence on our list).

## 5. Interop clients

| Client | Language | Latest version (date) | Licence | Maintained (source) | Reads CSDL XML `$metadata` | Smallest CI job shape (untested) |
|---|---|---|---|---|---|---|
| Microsoft.OData.Client (+ Microsoft.OData.Edm) | C# / .NET | 9.0.0 (NuGet published 2026-07-28), package targets `lib/net10.0` | MIT (nuspec `license` expression) | yes: OData/odata.net, last push 2026-09-02, 514 open issues+PRs | yes: `src/Microsoft.OData.Edm/Csdl/CsdlReader.cs` (and `CsdlReader.Json.cs` for CSDL JSON); client side `LoadServiceModel` in `src/Microsoft.OData.Client/DataServiceClientFormat.cs` | image `mcr.microsoft.com/dotnet/sdk:10.0`; `dotnet new console -o probe && dotnet add probe package Microsoft.OData.Client --version 9.0.0`; program: `CsdlReader.TryParse(XmlReader.Create(root + "$metadata"), out model, out errors)` must be true with no errors, then `new DataServiceContext(new Uri(root))` with `Format.UseJson(model)` (call names not verified) runs `GET <EntitySet>?$top=1` and checks one entity comes back |
| SAP Cloud SDK for JavaScript: `@sap-cloud-sdk/generator` + `@sap-cloud-sdk/odata-v4` | TypeScript / Node | 4.9.1 (npm, 2026-08-25) for both | Apache-2.0 | yes: SAP/cloud-sdk-js, last push 2026-10-10, 15 open issues+PRs | yes: generator README "create your own service module using a service specification (`.edmx` file)" | image `node:22` (engine field not read, unknown); `curl -o svc.edmx <root>/$metadata && npx -y @sap-cloud-sdk/generator@4.9.1 --input svc.edmx --outputDir gen`, then a script with `@sap-cloud-sdk/odata-v4@4.9.1` that runs `<entityApi>.requestBuilder().getAll().top(1).execute({ url: root })` on the generated API (API names not verified) |
| python-odata | Python | 0.8.1 (PyPI, 2026-08-04) | MIT (PyPI licence expression) | releases continue (0.7.0 2026-03-16, 0.8.0/0.8.1 2026-08-04); source repository unknown (PyPI lists no project URL; tuomur/python-odata last push 2024-02-28) | yes: `ODataService(url, reflect_entities=True)` reads `$metadata` (README); README says "Supports OData version 4.0" | image `python:3.12-slim`; `pip install python-odata==0.8.1`; script: `ODataService(root, reflect_entities=True)`, take one entity class from `service.entities`, `service.query(cls).limit(1).all()` (method names from memory, not verified) |

Seen and not counted:
- `@odata2ts/odata2ts` 0.45.0 (npm, 2026-09-22, MIT), TypeScript generator from metadata: a valid
  fourth client if one of the three drops out.
- Apache Olingo OData 4 Java client `org.apache.olingo:odata-client-core` 5.0.0 (Maven Central
  directory dated 2023-12-17), Apache-2.0, reads metadata via `XMLMetadataRequest`
  (`lib/client-api/src/main/java/org/apache/olingo/client/api/communication/request/retrieve/XMLMetadataRequest.java`).
  Its GitHub mirror apache/olingo-odata4 is archived (`archived: true`), so it is not counted as
  maintained.
- Simple.OData.Client (.NET): last stable 6.0.1 (2022-11-08), later only `6.1.0-CI-*` builds
  (last 2024-07-12); not counted.
- pyodata 1.12.1 (PyPI 2026-09-23, Apache-2.0): README "Supported features: OData V2", so it cannot
  read a V4 `$metadata`.

Recommendation: the .NET and SAP JavaScript clients as the two required jobs (two vendors, two
languages, both active), python-odata as a third, non-blocking job until its source repository is
confirmed.

## 6. Licences against `deny.toml`

Allow list (`deny.toml`, `[licenses] allow`): MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception,
BSD-2-Clause, BSD-3-Clause, EUPL-1.2, ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0. Versions and
licences are the latest stable on crates.io read today; "`unsafe` lines" is a grep of the crate
tarball's `src/` for `\bunsafe\b` (comment lines dropped).

| Crate | Version checked (date) | Licence | On allow list | In our `Cargo.lock` | `unsafe` | Role |
|---|---|---|---|---|---|---|
| quick-xml | 0.42.0 (2026-08-22) | MIT | yes | no | `#![forbid(unsafe_code)]` (`src/lib.rs:47`); depends on memchr | CSDL XML writer/reader |
| memchr | 2.8.3 (2026-07-08) | Unlicense OR MIT | yes (via MIT) | yes, 2.8.3 (`Cargo.lock:1924`) | yes (SIMD), not counted | multipart boundary search, quick-xml dep |
| serde_json | 1.0.151 (2026-07-20) | MIT OR Apache-2.0 | yes | yes, 1.0.151 (`Cargo.lock:3004`) | 13 lines | CSDL JSON, JSON format, JSON batch |
| serde | 1.0.229 (2026-07-18) | MIT OR Apache-2.0 | yes | yes (`Cargo.lock:2974`) | not counted | |
| nom | 8.0.0 (2025-01-26) | MIT | yes | yes, 8.0.0 (`Cargo.lock:2051`) | 4 lines | parser option A |
| winnow | 1.0.4 (2026-07-13) | MIT | yes | no | 44 lines | parser option B |
| chumsky | 0.13.0 (2026-05-06) | MIT | yes | no | 128 lines; transitive deps not checked | parser option C |
| logos | 0.16.1 (2026-01-30) | MIT OR Apache-2.0 | yes | no | 11 lines | lexer option |
| peg | 0.8.6 (2026-05-04) | MIT | yes | no | 0 lines in the facade crate; `peg-runtime` 0.8.6 (MIT) and `peg-macros` not grepped | parser option D (used by `cf-gears-toolkit-odata`) |
| percent-encoding | 2.3.2 (2025-08-21) | MIT OR Apache-2.0 | yes | yes, 2.3.2 (`Cargo.lock:2238`) | 4 lines | URL decoding |
| thiserror | 2.0.21 (2026-09-23) | MIT OR Apache-2.0 | yes | yes (`Cargo.lock:3515`) | not counted | error types |
| indexmap | 2.14.2 (2026-09-05) | Apache-2.0 OR MIT | yes | yes, 2.14.0 (`Cargo.lock:1735`) | not counted | ordered JSON members |
| multer | 3.1.0 (2024-05-04) | MIT | yes | no | `#![forbid(unsafe_code)]` (`src/lib.rs:112`); pulls tokio, futures-util, spin (MIT), encoding_rs ((Apache-2.0 OR MIT) AND BSD-3-Clause, allowed) | not chosen (3.4) |
| http | 1.5.0 (2026-07-29) | MIT OR Apache-2.0 | yes | yes (`Cargo.lock:1380`) | not counted | `odata-axum` only |
| axum | 0.8.9 (2026-04-14) | MIT | yes | yes, 0.8.9 (`Cargo.lock:378`) | not counted | `odata-axum` only |
| proptest | 1.11.0 (2026-03-24) | MIT OR Apache-2.0 | yes | no | not counted | dev |
| criterion | 0.8.2 (2026-02-04) | Apache-2.0 OR MIT | yes | no | not counted | dev (benches) |
| arbitrary | 1.5.0 (2026-10-06) | MIT OR Apache-2.0 | yes | no | not counted | fuzz input types |
| libfuzzer-sys | 0.4.13 (2026-06-04) | (MIT OR Apache-2.0) AND NCSA | no: NCSA is not on the list | `fuzz/Cargo.lock:1045`, already used (`fuzz/Cargo.toml:14`) | yes (FFI to libFuzzer) | fuzz only; the `fuzz` workspace is excluded (`Cargo.toml:21`), so the root `cargo deny` does not see it. Stays as today |

Parser choice (for the ADR): every combinator library above contains `unsafe` in its own source;
only `quick-xml` and `multer` forbid it. A hand-written recursive-descent parser for `odata-url`
following the ABNF rule by rule needs no parser dependency, carries the depth and node counters
directly, and gives byte offsets that match the OASIS `FailAt` values; `nom` 8.0.0 is the fallback
because it is already in our lock file. Any `unsafe` in a chosen dependency is named in the ADR, as
`tasks/GOAL-antares-surfaces.md` point 8 requires.

## 7. Unknown or not done

- lib.rs not searched (HTTP 403).
- `cf-modkit-odata`: repository `cyberfabric/cyberfabric-core` answers 404; coverage and `unsafe`
  unknown.
- GitLab `tobz1000/odata_client_rs`: open issues unknown (GitLab not queried).
- `unsafe` not checked for `parse-sap-odata`, `odata-rust-generator`, `odata-simple-client`,
  `reso-client`, `odata_client*`, `zkud/cim`.
- OData version not stated by cwoodruff/odata-rust-modular-monolith (no OData code found).
- python-odata source repository unknown.
- Node engine requirement of `@sap-cloud-sdk/generator` 4.9.1 unknown (field absent in the
  registry document read).
- A Rust JSON Schema validator that supports the `\p{L}` patterns of `csdl.schema.json`: unknown.
- Transitive dependencies of winnow, chumsky, logos and peg were not checked for licence or
  `unsafe`.
- Every CI job shape in sections 4 and 5 is a proposal; none was run.
