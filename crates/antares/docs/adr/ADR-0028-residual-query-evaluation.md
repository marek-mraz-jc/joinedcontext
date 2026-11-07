# ADR-0028 — Standard queries over a view: NGSI-LD pushdown plus a bounded residual

Date: 2026-10-11. Status: accepted. Decided by worker-1 under the owner
order of 2026-10-11 (via the supervisor): "every standard on top of
NGSI-LD serves its full query feature set while the Entities it serves
stay the ones the NGSI-LD view and its Policy admit". The owner may
override any rule below. Builds on ADR-0020 (policy seam), ADR-0023
(views), ADR-0024 (OData library) and ADR-0027 (surfaces). Task T-3748;
the per-standard analysis is `docs/analysis/standard-query-residual.md`.

## Context

A surface answers a read by running one NGSI-LD Query Entities request
through `ViewOps::run` (`crates/antares-ops/src/lib.rs:438`), which runs it
in the view's Tenant under the view's narrowing and the policy engine, and
hands back the broker's HTTP answer. OData builds that request in
`crates/antares-odata/src/read.rs:263` (`query`) from the translation in
`crates/antares-odata/src/filter.rs` (`translate`, `order`). Whatever
`filter.rs` cannot say in `q`, `geoQ`, `id` or `orderBy` is refused before
any operation runs: `Refused::Later` is 501 (`filter.rs:36-44`).
`odata-linkml-mapping.md` lists 21 such rows (F11, F13, F14, F16 to F19,
F23, F25, F27, F30, F33 among the filter rows; Q14, Q16, Q19 among the
options). The gateway's OGC surface has the same shape: CQL2 compiles to
`q`/`geoQ` or is refused (platform `crates/context-gateway/src/translators/cql2.rs`;
cg: only `handlers::ogc::ogc_items` calls `cql2::compile`).

Paging and counting pass straight through. A collection page asks for
`limit={page}&offset={skip}&count=true` and the skiptoken carries the
NGSI-LD offset (`read.rs:352-372`, `read.rs:572`). `$count` and `/$count`
are the `NGSILD-Results-Count` of the same query (`read.rs:429-457`).
Once some of the filter is evaluated outside the broker, an NGSI-LD offset
is no longer an offset into the standard's answer, and the NGSI-LD count
is no longer its count. This ADR fixes how both stay exact.

The view is the security boundary, and it already holds the rule this
design needs. ADR-0023 rule 2: "a term whose Attribute path starts at a
hidden Attribute ... is evaluated as if the Entity did not contain it",
and "`attrs`/`pick` naming one yield nothing for it". A hidden Entity is
never in the answer (ADR-0023, "a caller cannot tell a hidden entity from
an absent one").

## Decision

### 1. Pushdown first, residual for the rest

Each surface compiles its query into two parts:

- **pushdown**: the largest NGSI-LD Query Entities request that selects a
  superset of the answer: `type`, `id`, `q`, `scopeQ`, `geoQ`, `attrs`
  (`pick`/`omit`), `orderBy`. Only constructs whose NGSI-LD meaning equals
  the standard's meaning are pushed: the mapped rows of
  `odata-linkml-mapping.md` 5.2 are that list for OData.
- **residual**: everything else, as one expression of the shared
  intermediate form (rule 2), evaluated on each Entity the pushdown
  returned.

The split is by top-level conjunct. A conjunct pushes as a whole or goes
to the residual as a whole; a disjunction with any unpushable member is
residual as a whole. The answer is pushdown ∩ residual, and since every
pushed conjunct means the same in both languages, that is exactly the
standard's answer over what the view shows.

A query with an empty residual runs exactly as today: one request, the
broker's count, offset paging. The residual path costs nothing for a
query that does not need it.

### 2. One evaluator crate, reached through antares-ops

A new pure crate `antares-residual` holds the intermediate form (`Expr`:
paths into an NGSI-LD Entity, literals, comparison, the three-valued
`and`/`or`/`not`, function calls from a closed table, lambdas over a
list) and its evaluator over a normalized NGSI-LD Entity (`serde_json`
only, no I/O, `#![forbid(unsafe_code)]`, Miri-clean per GOAL point 8).
Every surface compiles into it; none evaluates on its own.

The fetch loop lives in `antares-ops` as `ViewOps::scan`, which re-exports
the crate, so a surface still depends only on `antares-ops` and its
protocol crates (ADR-0027 rule M2).

Decided (worker-1, owner may override): a separate pure crate re-exported
by `antares-ops`, not a module inside it, because GOAL point 8 runs Miri
on the pure evaluator crates and `antares-ops` links the broker; options:
a module of `antares-ops` (the task's wording, no Miri), a crate per
standard (three evaluators of one semantics).

Evaluation uses three-valued logic. A missing Attribute, a hidden one and
a value of the wrong type are all `unknown`; the top level keeps an Entity
only when the expression is `true`. Each standard's compiler maps its own
null rules onto this (OData URL Conventions 5.1.1.1.2: "The null value
is not equal to any value but itself", and 5.1.1.1.3: "If any operand is
null, the operator returns false"; CQL2 through its `IS NULL`). `not` is evaluated exactly, which is what makes F11 servable:
the mapping noted De Morgan does not hold under 7.2.3.3's type-mismatch
rule, and the residual does not need it to.

### 3. The residual never widens

`ViewOps::scan` evaluates only on Entities `ViewOps::run` returned for the
caller, after the view's narrowing and the engine's decision. It drops
Entities; it never adds one, and never adds an Attribute the answer did
not carry. A hidden Attribute is absent from that answer (ADR-0023
rule 2), so a residual term on it evaluates as on an absent one, the same
answer the pushdown gives: no query can tell "hidden" from "absent" by
moving a term from `q` into the residual.

The pushdown asks for what the residual reads: `pick` = `id`, `type`,
`scope`, the standard's projection ∪ the Attributes the residual and the
ordering name (T-3822: `pick`, never `attrs`, because `attrs` also selects:
an Entity holding none of them would leave the answer and its count), and
`options=sysAttrs` when it names `createdAt`/`modifiedAt` (F33). The
surface projects to the standard's selection after evaluation. A value
the residual read but the caller did not select never leaves the process.

### 4. Paging: an upstream cursor, filled per request within a budget

With a residual, the surface pushes an `orderBy` that ends in `id`
(TS 104 176 7.2.3.1 allows a list and names `id` as a member), so the
upstream order is total and stable. A page is filled by reading upstream
pages of the broker's `max_limit` (1000, `crates/antares-api/src/state.rs:488`)
from the cursor, keeping the Entities the residual admits, until the
page holds its size or the upstream answer ends.

The page's continuation (OData skiptoken, OGC `next` link) carries the
upstream offset just past the last Entity it served, the remaining
`$top`, and the request digest the skiptoken already binds (`read.rs:555-593`).
Every page continues exactly where the last one stopped, with no gap and
no repeat. Concurrent writes move it the way they move NGSI-LD offset
paging today (TS 104 175 7.4.2.1: implementations "may warn" when result
sets change while paging).

Each request reads at most `RESIDUAL_SCAN = 10 000` upstream Entities. When
the budget runs out before the page is full, the page is served with what
it holds plus its continuation. Server-driven paging allows a short page
(OData Protocol 11.2.6.7; OGC `numberReturned` < `limit` with `next`), and
the page is complete, not partial: every Entity on it is in the answer and
the continuation reaches the rest.

Decided (worker-1, owner may override): a constant of 10 000 per request,
10 upstream pages of the broker's `max_limit`, with a `ponytail:` comment
naming the upgrade path (an `ANTARES_RESIDUAL_SCAN` variable when a
deployment measures that it needs another value); options: an environment
variable now, a per-view value.

### 5. Count and residual ordering: exact within the budget, refused beyond it

`$count`, `/$count`, OData `$orderby` and OGC `sortby` on a key the pushdown
cannot order by need the whole filtered set. With a residual, the surface
reads the pushdown's `NGSILD-Results-Count` first. When it is at most
`RESIDUAL_SCAN`, the scan reads all of it, then counts and sorts exactly.
When it is larger, the request is refused before any Entity is
evaluated:

- OData: 400, code `TooManyResults`, message naming the budget and the
  pushdown's size;
- OGC: 400 with the same reason for a `sortby`. `numberMatched` is
  optional in OGC API Features, so it is omitted rather than refused.

Nothing is counted approximately, and no page is served from a partial
sort.

A pushed OData `$orderby` follows URL 5.1.5 too: a missing value comes
first ascending and last descending. The store orders a missing Attribute
after every value, so the read first asks one count of the Entities that
lack a sort key, `q=(<q>);(!k1|!k2…)`. A count of 0 keeps the pushed sort
and its paging. Any other count turns the read into the surface sort
above, within `RESIDUAL_SCAN` and refused beyond it (T-3853).

Decided (worker-4, owner may override): count first, then sort on the
surface only when a key is missing somewhere, because one count costs
less than a scan and leaves the common case pushed; options: split a
pushed sort into "lacks the key" and "holds it" reads (right for the
first key only, 2^k reads for k keys), always sort on the surface (a
scan for every `$orderby`), keep the store's order (contradicts URL
5.1.5 and the surface sort of `createdAt`).

Decided (worker-1, owner may override): 400 with the NGSI-LD error name
`TooManyResults`, because the caller fixes it by narrowing the query and
OData already answers `TooComplexQuery` with 400 (`filter.rs` `Refused::TooComplex`);
options: 403 (the NGSI-LD binding's status for `TooManyResults`,
`crates/antares-model/src/error.rs:77`, read by OData and OGC clients as an
authorization failure), 413 (RFC 9110 reserves it for request content),
409 (no conflicting state).

### 6. NGSI-LD options beside a standard's own

Every standard surface accepts the NGSI-LD Query Entities options under
one prefix, `ngsild.`: `ngsild.q`, `ngsild.scopeQ`, `ngsild.georel`,
`ngsild.geometry`, `ngsild.coordinates`, `ngsild.geoproperty`,
`ngsild.pick`, `ngsild.omit`. Values are NGSI-LD syntax (TS 104 176
7.2.3.1) with terms resolved against the view's `@context`. They are ANDed
into the pushdown:

- `ngsild.q` joins the surface's own `q` with `;`;
- `ngsild.scopeQ` is pushed as is;
- `ngsild.geo*` takes the request's one `geoQ`, and a standard geo
  conjunct then goes to the residual (F29 stays exact);
- `ngsild.pick`/`ngsild.omit` narrow the pushdown's `pick`. They also
  narrow what the residual can read: a filter on an omitted Attribute
  evaluates as absent, like a hidden one. Every `ngsild.` option narrows.

In OData they are custom query options (URL Conventions 5.2: no `$`, no
`@`). Row Q27 ("custom query options: 400") gains this list as the only
names accepted, and every other custom option stays 400. In OGC API
Features they are parameters the collection's OpenAPI declares.

A broken `ngsild.` value is the broker's own 400 (`BadRequestData`),
raised by the pushdown request.

### 7. Bounds

The evaluator caps expression depth (64) and node count (4096) at compile
time; an expression past either is 400 `TooComplexQuery`, matching
`filter.rs` `MAX_Q_BYTES`. String functions are bounded by the value they
read. `matchesPattern` and CQL2 `LIKE` compile with the `regex` crate,
which matches in linear time; a pattern using a construct outside the
ECMAScript subset `regex` shares (look-around, back-references) is 501
`NotImplemented`, never a different meaning (the F16 note). Arithmetic is
checked; an overflow is `unknown`, not a wrap.

### 8. What stays 501, and why

| construct | standard | reason |
|---|---|---|
| `$deltatoken`, change tracking (Q21) | OData | NGSI-LD has no change feed for a query; a Subscription is another contract |
| `$index` (Q17) | OData | a positional insert, a write with no NGSI-LD operation |
| `$root` (Q24) | OData | a second read inside a filter: one gated read per evaluated Entity, unbounded fan-out |
| `$crossjoin`, `$all` (Q36) | OData | a cross product of sets, quadratic in the budget |
| `geo.distance`, `geo.length` (F30) | OData | URL Conventions 5.1.1.11.1 leaves the unit for SRID 4326 unstated; an answer would pick one |
| a navigation path in a filter (F24) | OData, CQL2 | stays gated on the linked-entity checks (T-3470, ADR-0023 rule 6); not this ADR's to open |
| regex constructs outside the shared subset | OData, CQL2 | rule 7 |

Everything else in the mapping's 501 rows is served by the residual, and
the per-standard slices (section 6 of the analysis) carry them.

## Consequences

- OData `$filter`, `$orderby`, `$search`, `$compute` and `$apply`, and the
  OGC CQL2 classes, become servable over any view without a new store
  capability. The view and the engine stay the only gate.
- A residual query costs up to `RESIDUAL_SCAN` evaluated Entities per
  request, and a count or residual sort costs one extra counted request
  first. Queries that push down fully are unchanged.
- The OData skiptoken format changes for residual queries (upstream cursor
  in place of an offset). Tokens are opaque and digest-bound, so an old
  token is refused as `bad_token`, never misread.
- `odata-linkml-mapping.md` rows move from "rejected 501" to "residual" as
  each slice lands, docs first.

## Confirmation

Property tests, one per guarantee, named for the slices that build them:

| guarantee | test | crate |
|---|---|---|
| residual answer = brute force over the view's full answer | `residual_equals_brute_force` | `antares-residual`, `antares-ops` |
| the residual never returns an Entity or Attribute the view hides | `residual_never_widens` (a narrowing engine hides random Entities and Attributes) | `antares-ops` |
| paging is stable: the pages' concatenation equals the unpaged answer for any page size and budget | `residual_pages_concatenate` | `antares-ops` |
| no request reads more than `RESIDUAL_SCAN` upstream Entities; over-budget count or sort is 400, never a partial body | `residual_scan_is_bounded` | `antares-ops` |
| an `ngsild.` option only narrows: answer = standard answer ∩ NGSI-LD answer | `ngsild_options_narrow` | `antares-odata`, `antares-ogc` |
| a filter on a hidden Attribute answers as on an absent one, pushed or residual | `hidden_attribute_is_absent_in_residual` | `antares-ops` |
