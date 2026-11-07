---
title: NGSI-LD view and policy filtering, candidates for a better path
task: T-3778
---

# NGSI-LD view and policy filtering: candidates for a better path

Where a view's and a policy's `q`, `geoQ` and `scopeQ` reach the store and
where they are judged in memory, the candidates the owner order of
2026-10-11 named, and which one was adopted with its proof. A candidate is
adopted only when the T-3777 matrix and its differential stay identical and
a named shape is measured faster.

## 1. Code relations behind this analysis

Graph: Antares dev `f143863`, built 2026-10-11T21:08Z. Rerun from a clone
after `cg build --reuse`:

```cypher
CALLERS intersect_scope_q;
CALLERS compile_scope_q;
REACH narrow_repr;
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::views::compose'}) WHERE NOT c.is_test RETURN DISTINCT c.path;
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_api::views::with_view'}) WHERE NOT c.is_test RETURN count(DISTINCT c);
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_ql::geo::GeoQuery::to_sql_spec'}) WHERE NOT c.is_test RETURN DISTINCT c.path;
```

Results:

- `intersect_scope_q` has two production callers, `policy::Filter::narrow_params`
  (once per request, on the request's own `scopeQ`) and `views::and_scope`
  (once per pairing of a view with an engine).
- `compile_scope_q` has one production caller, `PgEntityStore::query`: the
  narrowed `scopeQ` reaches SQL as anchored regexes with bound values.
- `narrow_repr` reaches `repr::reach_of`, `type_rules`, `fill` and the
  policy context's `expand_key`; `type_rules` already keeps its last
  expansion per thread by value (T-3667).
- `views::compose` has one caller, `views::with_view`, which the `gate!`
  macro calls from 56 handlers: every request through a view under an
  engine that narrows pairs the two once.
- `GeoQuery::to_sql_spec` is called by `entities::filter_entities_paged`
  and `notify::interval_tick`.

The task's `IMPACT compose;` matches every symbol named `compose` (router
composition included) and says nothing about the filter; the path query
above replaces it. `TESTS selector_match;` lists 60+ tests of the selector
grammar and is not repeated here.

## 2. Where each part of a narrowing is judged

| Part | Query Entities | Temporal query | Subscriptions |
|---|---|---|---|
| view and policy `q` | conjoined with the request's `q` on the AST, sent to the store as `q` (`narrow_params`) | the same, judged on the instances inside the interval | judged in memory per notification |
| `scopeQ` | intersected once per request, compiled to SQL | the same | in memory |
| `geoQ` (every area) | compiled to SQL with the request's own (`filter_entities_paged`, `to_sql_spec`); an area that does not compile forfeits every pushdown, so no page is cut on the wrong set | judged on the instances inside the interval (CIM 009 5.7.4.4) | in memory, and on the interval tick |
| `pick`/`omit`, per-type rules | projection after the read (`narrow_repr`); rules that select by type alone go to the store as a type selection (T-3667) | the same | the same |

Every surface (OData, OGC, LinkML, MCP, WebSocket, NGSI-LD under
`/x/views`) runs its operations through `ViewOps::run` and `Ops::run_in`
into `AppState::call`, so all of them read one composed filter on the same
handler path (the T-3844 trace proves no surface reaches the store past
`antares-ops`).

## 3. Candidates

| # | Candidate | Verdict | Evidence |
|---|---|---|---|
| 1 | Keep the pairing of a view with an engine's narrowing per thread, keyed by the whole input | **adopted** | section 4 |
| 2 | Push the view's `geoQ` into PostGIS instead of filtering after the fetch | already in place | `filter_entities_paged` compiles every area, the view's included; temporal and notifications judge areas on what the spec says they read |
| 3 | Intersect several policies' `scopeQ` once per request, not per Entity | already in place | `intersect_scope_q` runs in `narrow_params` per request and in `and_scope` per pairing; no per-Entity caller |
| 4 | Cache the composed AST per view revision | adopted as part of 1 | the key holds the view's `Arc`, which a reload replaces |
| 5 | One composed filter shared by all surfaces | already in place | section 2, last paragraph |
| 6 | Hand the store the composed `q` AST instead of its text | not built | the text is also the query a federated request forwards and the one the local re-check of 5.7.2.4 parses; after 1 the parse is the only per-request cost left of the pairing, and it is not measured yet |
| 7 | NGSI-LD `q` extensions (projection, LanguageProperty and datasetId selectors) so more of OData and CQL2 pushes down | not built | a spec-level change needs its docs and ADR first (task step 4); `standard-query-residual.md` lists what each surface leaves to the residual today |

## 4. Candidate 1: the kept pairing

`gate` answers every request with a fresh `Filter`, so under an engine that
narrows, `with_view` composed the view with it on every request, and
`narrow_repr`'s pointer-keyed memo missed every time. The pairing is a
function of three inputs: the view's narrowing, the clause and the
engine's `Filter`. `views::paired` keeps the last answer per thread with
that whole input as its key: the view's `Arc` (held, so its address cannot
be reused), the clause, and the engine's `Filter` compared by value. A
different narrowing, clause or reloaded view is paired afresh; a filter
unequal to itself (a NaN) never hits. A hit returns the same `Arc`, so
`narrow_repr` finds its reach by pointer too.

Measured with `cargo bench -p antares-ops --bench views` on the shared
12-core host (load 35 to 41), three runs each, nanoseconds per request:

| Shape | before | after |
|---|---|---|
| query through the largest view (`MAX_RULES` rules), narrowing engine | 62 550, 65 379, 71 282 | 268, 256, 284 |
| query through a one-type view, narrowing engine | 1 209, 1 204, 970 | 239, 224, 234 |

The `after` figure includes the clone of the engine's `Filter` that the
bench makes per request, as `gate` does. An allow-all engine never reaches
the pairing (T-3697), so its path is unchanged.

Proof of equivalence: `views::tests::the_kept_pairing_answers_only_its_own_input`
(red before: two requests got two `Arc`s), the T-3777 scenario matrix, and
its differential at 20 000 cases (`VIEWS_DIFF_CASES=20000`), all green on
the change. The run is named in the T-3778 task body.

## Related

- T-3777, the matrix and differential every candidate is held to
- `standard-query-residual.md`, the residual each standard surface serves
- `surfaces-threat-model.md`, threat O16 (a memo never answers for another input)
- ADR-0023, how a view and a policy compose
