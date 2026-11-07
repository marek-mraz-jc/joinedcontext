# One NGSI-LD read across many views: mapping note (T-4494)

Sources: ETSI GS CIM 009 / TS 104 175 V1.9.1 clauses 4.12 (pagination), 4.23.1 (ordering of distributed
operations), 5.7.2 / 10.4.3 (Query Entities), 6.3.10 (limit, count); ADR-0023 (views); ADR-N-050. The code is
`crates/antares-views/src/union.rs`; the user page is [views.md section 6](../src/views.md#6-one-read-across-views).

## 1. The question

The public Endpoints on dev are views on one broker. A type lives in many of them: Event in 7,
KeyPerformanceIndicator in 26, AirQualityObserved in 6. Asking for every Event this weekend took one Query Entities
per view. The union answers it in one request, and each part of the answer is exactly what that view would have
answered on its own.

## 2. Two designs

| | Union route in the view surface | A Context Source Registration per view (antares-icsr) |
|---|---|---|
| Moving parts | one route; each part is `ViewOps::run(QueryEntities)` as `/x/views/{view}` runs it | one registration per view to keep in step with the view table, the distributed-query fan-out, and a loop-back to the broker itself |
| Policy | each part runs inside `through_view`, as a direct read through that view does | the fan-out calls each view's root over HTTP and carries the caller's credentials across a hop |
| Tagging | the union knows which part each Entity came from | merged Entities lose their source (4.5.5.3 merges by id) |
| Paging | one cursor over per-view offsets | distributed queries never order (4.23.1), and pages over a merged set are not stable |

Decided (worker-1, owner may override): the union route. It has fewer moving parts, it reuses the view's own path
for every part, so "each part through its own view and Policy exactly as a direct read" holds by construction, and
it can say which view each Entity came from. A registration per view would merge Entities by id, which is the
cross-view join the security requirement rules out.

## 3. The operation

`GET /x/views/_union/ngsi-ld/v1/entities?views=a,b,c&…`. `_` is no view-id character (`[a-z0-9-]`), so no view
shadows the route. The gateway resolves "all I may read" into the `views` list. The broker never lists the views a
caller may read, because that would disclose the other views.

- **Parameters.** Every Query Entities parameter but those below goes to each view unchanged: `type`, `q`, the
  geo-query, the temporal window of the `q`, `attrs`, `pick`, `omit`, `options`, `lang`, `local`. Each view applies
  its own selector, rules and narrowing to it.
- **Refused with 400:**
  - `orderBy` and `collation`: the union is a distributed operation, and 4.23.1 says sort ordering is never applied
    to one.
  - `offset` other than 0: the next page is reached through the cursor.
  - `entityMap`, the `NGSILD-EntityMap` header, and `NGSILD-Tenant`: each view reads its own Tenant, and a union
    names none.
  - More views than `ANTARES_UNION_MAX_VIEWS` (default 200).
  - A `limit` above 1000.
  - A cursor that does not fit the `views` list.
- **Order.** The order is by Entity id, then by the position of the view in `views`. Each view's pages come in id
  order (the store's `ORDER BY id`), and the union merges the heads. An Entity two views show appears once per view,
  each part tagged with its own view. The union never merges the two.
- **Paging.** The `limit` defaults to 100. The next page is a `Link` with `rel="next"` whose `cursor` holds how far
  each view has been read, as dot-separated offsets in the order of `views`. A cursor is only useful with the same
  `views` list. A cursor someone edited still reads each view through its own rules, so it can skip or repeat
  Entities but never show one the view hides. No `prev` link is given (4.12 makes it a should): an offset per view
  cannot be read backwards without reading the views again. A `next` link can point at an empty last page when a
  view had exactly as many Entities left as the union asked it for.
- **Count.** With `count=true`, `NGSILD-Results-Count` is the sum of the views' counts. Every view is asked for its
  count, and a view that contributes nothing adds nothing.
- **Bounds.** The union buffers at most 2000 Entities across all views at once. Each view is asked for
  `min(limit, 2000 / views)` Entities per page, and is asked again when its buffer runs out. At most 16 views are
  asked at the same time. A page from one view larger than 32 MiB makes the union answer 400. Each part is also held to
  the broker's own budgets (`limit`, the `RESIDUAL_SCAN` of a conditional count, the batch and lane bounds), as a
  direct read is.
- **Writes.** Every method but `GET` and `HEAD` answers 405 with `Allow: GET, HEAD`.

## 4. A view the caller may not read

A view that is unknown, does not allow `query_entities`, or whose Policy refuses the caller (401, 403 or 404)
contributes nothing: no Entity, no count, no id, and no sign of which view refused. A request
that is wrong for every view alike (a malformed `q`, an unknown geometry) answers with the first such view's 400. A
failure inside the broker answers 500 with a request id, and a view's 503 answers 503. The union never answers a
partial page as if it were complete.

## 5. The view of each Entity

TS 104 175 has no member that names where an Entity came from. A Property added to each Entity would be read as an
Attribute of the Entity (the core @context has an `@vocab`) and would collide with a real Attribute of that name.
The body therefore stays exactly as the views answered it. The response header `Antares-Entity-Views` lists one view
id per Entity, in body order. A view id is `[a-z0-9-]`, so the list is plain comma-separated text.

## 6. @context

Each view is asked for `application/ld+json`, so every Entity carries its own `@context`. Every view compacts its
answer under the same context: the caller's `Link` if it sent one, otherwise the core context. A view of another
model (G18) answers under the core context too.

| Request | Answer |
|---|---|
| `Accept: application/ld+json` | the Entities as the views answered them, each with its own `@context` |
| `application/json`, no `Accept`, `*/*` or `application/*` | each Entity's `@context` is removed and named once, in a `Link` header |
| any other `Accept` | 406, naming `application/ld+json` |

## 7. Data sources

- The tests use the view fixture of `crates/antares-views/tests/views/common` (Cars and a Person in tenant `city`)
  and three views over Vehicles split by id.
- On dev, the 7 Endpoints that hold Event are the target: the gateway passes their view ids as `views`.

## Related

- [Endpoint views](../src/views.md)
- [view-mapping.md](view-mapping.md): views of another model
- [operation-matrix.md](operation-matrix.md)
