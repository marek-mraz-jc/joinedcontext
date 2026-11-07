# Admin API

This chapter is every route the broker serves OUTSIDE `/ngsi-ld/v1`.
There are three groups and no others: `/q/` is the operator surface,
`/ex/v1/` is the peer-facing wire between brokers, and `/x/` is whatever a
deployment mounted. Everything else the broker answers is CIM 009, and the
[conformance ledger](conformance.md) owns it.

`/q/` carries no NGSI-LD semantics and belongs behind the gateway. A worker
pod (`ANTARES_ROLES` without `api`) serves only these routes. Errors use the
same problem-details shape as the NGSI-LD API.

A deployment can also put `/q/` behind a secret of the broker's own, so a
network position is not the only thing between a caller and a tenant purge.
`ANTARES_ADMIN_TOKEN_FILE` names a file holding it (a mounted Secret, at
least 32 bytes, one trailing line break ignored). With it set, every `/q/`
route except `GET /q/health` and `GET /q/ready` answers `401` with
`WWW-Authenticate: Bearer realm="antares-admin"` and the problem type
`urn:antares:error:Unauthorized` unless the request carries
`Authorization: Bearer <secret>`; the comparison does not stop at the first
differing byte. `GET /q/metrics` is guarded too unless
`ANTARES_ADMIN_METRICS=open`, for a scraper that cannot send a header.
`ANTARES_METRICS_PORT` moves `GET /q/metrics` to a listener of its own that
serves nothing else, so the scraper needs no path to the API port. The
broker reads the file once at start, refuses to start when it is unreadable
or too short, and never logs the secret. Unset, `/q/` asks for nothing and
the broker logs one warning at start saying so.

The same routes as an OpenAPI document, with every status code and response
schema: `docs/openapi/antares-admin.yaml`, rendered at
<https://antaresbroker.joinedcontext.com/openapi/admin.html> (see the
[API reference](api.md)).

| Route | Purpose |
|---|---|
| `GET /q/health` | Liveness and the process view |
| `GET /q/ready` | Readiness for the load balancer |
| `GET /q/metrics` | Prometheus text |
| `GET /q/tenants` | Tenant names |
| `GET /q/tenants/{tenant}` | What one tenant holds |
| `DELETE /q/tenants/{tenant}` | Tenant purge |
| `GET /q/tenants/{tenant}/digest` | Entity count, history count and a hash over (id, `modifiedAt`) |
| `GET /q/tenants/{tenant}/export` | The tenant's Entities and history as NDJSON, system attributes kept |
| `POST /q/tenants/{tenant}/import` | An export into an empty tenant, all or nothing |
| `GET /q/tenants/{tenant}/types/{type}/attributes` | The Attribute names one type's Entities hold, no values |
| `GET /q/dead-letters` | Notifications the delivery policy gave up on |
| `GET`, `PUT`, `DELETE /q/retention/{tenant}` | How long a Tenant keeps each type ([Retention](#retention)) |
| `POST /q/retention/{tenant}/preview`, `GET /q/retention/{tenant}/deleted` | What a retention document would delete now; what this pod's sweeps deleted ([Retention](#retention)) |
| `POST /q/dead-letters/{id}/replay` | One more delivery attempt |
| `DELETE /q/dead-letters/{id}` | Drop a dead letter |
| `POST /ex/v1/remote-notify` | The 5.8.1.4 receiver other brokers post to |

## GET /q/health

`200` with the process view, `503` while the instance drains (a roll in
progress). A memory-store broker answers:

```json
{
  "status": "UP",
  "store": "memory",
  "temporal": "memory",
  "version": "0.1.0",
  "commit": "3432674",
  "notificationSchemes": ["http", "https", "mqtt", "mqtts"],
  "deadLetters": 0,
  "changesDropped": 0,
  "taskPanics": 0,
  "temporalDrainErrors": 0,
  "policy": { "engine": "allow-all", "timeoutMs": 250 },
  "surfaces": { "admin": { "prefix": "/q", "routes": 8 } },
  "limits": {
    "batchInflight": 8,
    "batchInflightBytes": 4194304,
    "changeQueue": 1024,
    "defaultLimit": 100,
    "deliveryLane": 64,
    "deliveryLaneBytes": 134217728,
    "deliveryWidth": 64,
    "deliveryWidthPerTenant": 8,
    "maxAnswerBytes": 67108864,
    "maxBatchItems": 1000,
    "maxBodyBytes": 4194304,
    "maxContextFetches": 32,
    "maxFedFanout": 8,
    "maxFedInflight": 256,
    "maxFedResponseBytes": 16777216,
    "maxFoldDocs": 100000,
    "maxGeoVertices": 1024,
    "maxInProcessCallDepth": 8,
    "maxJoinLevel": 10,
    "maxJsonDepth": 64,
    "maxLimit": 1000,
    "maxPeerWarnings": 8,
    "maxQLinkLookups": 512,
    "maxQNodes": 512,
    "maxRegexCache": 1024,
    "maxRegexCacheBytes": 67108864,
    "maxRegexProgramBytes": 262144,
    "maxTrackedDestinations": 4096,
    "maxUriBytes": 8192,
    "rejectedBodyTooDeep": 0,
    "rejectedBodyTooLarge": 0,
    "rejectedUriTooLong": 0,
    "temporalDefaultInstances": 100,
    "temporalInstanceLimit": 1000
  },
  "memory": { "allocatedBytes": 5671592, "residentBytes": 24051712 }
}
```

| Field | Meaning |
|---|---|
| `status` | `UP`; `DRAINING` with status 503 once shutdown began; `STORE_WEDGED` with status 503 when every Postgres pool slot stayed held for 5 min while the database answers a fresh connection (the liveness probe then restarts the pod). |
| `store` | The current-state backend: `memory`, `file`, `postgres`, `timescale`. |
| `temporal` | The history backend: one of the four, or `none` when history is off (`ANTARES_TEMPORAL`). |
| `storeInfo`, `temporalInfo` | What each driver actually runs on: `{engine}` for the built-in stores (`memory` or `redb`), and on Postgres `{engine, poolSize, poolWriteReserve, poolAcquireTimeoutSeconds, server, postgis?, timescaledb?}` — the pool's own shape always, the server probe read once at startup and omitted if it failed. Absent when a driver has nothing to add to its name (`none` history). Two deployments answering `postgres` are told apart here. |
| `version`, `commit` | Workspace version and the git hash the binary was built from. |
| `notificationSchemes` | The `notification.endpoint.uri` schemes this build can deliver to — the registered bindings (6.3.8, clause 7, and any a deployment added). A subscription naming a scheme absent here is refused at creation with `BadRequestData`. |
| `deadLetters` | Dead letters this process wrote since start ([notification delivery](operations.md#notification-delivery)). |
| `changesDropped` | Changes the bounded matcher queue refused since start, each one a notification never matched: delivery is slower than the write rate. |
| `taskPanics` | Panics absorbed at the notification-task boundary since start, each one a lost notification. Reported here because the Prometheus recorder is off unless `ANTARES_TELEMETRY` is set. |
| `temporalDrainErrors` | Post-response history writes that failed since start; the client's 2xx stands, the counter and a warning record the loss. |
| `policy` | The policy engine this binary was started with (`ANTARES_POLICY`, `allow-all` unless a deployment registered another) and the deadline one decision gets (`ANTARES_POLICY_TIMEOUT_MS`). An engine that overruns it is a refusal, not a delay. In a `views` build, `documents` lists the engine's rule documents by tenant and revision, `loaded` or `refused` ([Policy rule documents](#policy-rule-documents)); `null` when the engine reads none. |
| `surfaces` | The mounted admin surfaces by name, each with its prefix and route count. |
| `limits` | The bounds wall: every `max*` cap in force, the page defaults (`defaultLimit`, `temporalDefaultInstances`: what a request naming no `limit` or `lastN` gets) and the `rejected*` counters of requests refused by it, plus the notification-pipeline ceilings (`changeQueue`, `deliveryLane`, `deliveryWidth`, `deliveryWidthPerTenant`, `maxTrackedDestinations`). No request is rejected against those five — they are published because reaching one is what a dropped change or a stalled fan-out looks like from outside. Each subscription is delivered on a lane of its own, `deliveryLane` groups deep: a slow or dead endpoint fills its own lane, which drops for that subscription alone (counted in `antares_notification_lane_dropped_total`, logged with tenant and subscription), and never holds the matching of other subscriptions' changes. |
| `memory` | jemalloc live (`allocatedBytes`) and resident (`residentBytes`) bytes; `answerPool` the bytes buffered in-process answers hold (`heldBytes`) of the pool `ANTARES_MEMORY_BUDGET` gives them (`capBytes`); `contextCaches` the `hits` and `misses` of the merged and fetched @context caches since start. |
| `commitQueueDepth`, `commitQueuePeak` | Present only for a store with a single committer to queue behind (`file`, and the browser build over OPFS): writers queued now and at peak. |
| `bus` | `ANTARES_BUS=nats` only: `{mode, connected, reconnects}`. |

## GET /q/ready

`200 {"status":"READY","store":true}` when the instance is not draining,
the store answers (`SELECT 1` on Postgres) and, under `bus=nats`, the bus
is connected; otherwise `503 {"status":"NOT_READY", …}` with the failing
member `false`. Point the readiness probe here and the liveness probe at
`/q/health`: a restart does not cure a lost database.

## GET /q/metrics

Prometheus text with the `antares_` prefix:

| Metric | Meaning |
|---|---|
| `antares_http_requests_total` | Requests served, by method and status class. |
| `antares_http_request_duration_seconds` | Request service time histogram. |
| `antares_limit_rejections_total` | Bounds-wall rejections, by limit. |
| `antares_commit_queue_depth` | `file` store: writers queued behind the redb committer. |
| `antares_memory_allocated_bytes`, `antares_memory_resident_bytes` | jemalloc live and resident bytes. |
| `antares_context_cache_lookups_total{cache, result}` | @context cache lookups: `cache` is `merged` or `fetched`, `result` is `hit` or `miss`. |
| `antares_answer_pool_held_bytes` | Bytes buffered in-process answers hold of the memory budget's pool. |
| `antares_answer_pool_peak_bytes` | The most bytes the pool held at once since the previous 5 s sample. |
| `antares_answer_bytes{surface}` | Size histogram of each in-process answer read whole through the pool (`temporal`, `snapshots`, `scan`, `mapped`, `odata`, `sta`, `ogc`, `views`). |
| `antares_answer_pool_wait_seconds{surface}` | Time a buffered read took, its wait for room in a full pool included. |
| `antares_answer_pool_refusals_total{surface}` | Buffered reads answered 503 because no room freed within the 2 s wait. |
| `antares_uptime_seconds` | Seconds since process start. |
| `antares_draining` | 1 while this instance drains. |
| `antares_temporal_drain_errors_total` | Failed post-response history writes. |
| `antares_notifications_sent_total`, `antares_notifications_retried_total`, `antares_notifications_failed_total` | Notification deliveries by outcome. |
| `antares_notification_changes_dropped_total` | Change events the notifier dropped under back-pressure. |
| `antares_notification_lane_dropped_total` | Entities a subscription's full delivery lane dropped: that subscriber is slower than its changes (the log line names tenant and subscription). |
| `antares_claim_check_unresolved_total` | Changes too big for the bus whose kept outbox row was already reaped: the before-image is gone and the change notifies nobody. Non-zero means a matcher lagged past the retention window. |
| `antares_retention_deleted_total` | Entities (`kind="entity"`) and attribute instances (`kind="history"`) the [retention](#retention) sweep deleted, by `tenant` and `type`. |
| `antares_views_doc_reads_total` | Reads of one stored view, schema, retention, quality, policy or grant document by its id, by `by`: `reload` (a body whose digest moved since the last reload), `admin` (a GET on `/q/views/{id}`, `/q/schemas`, `/q/retention`, `/q/quality` or `/q/policy`, or a PUT on `/q/policy`) or `grant` (the export grant a delivery checks). An unchanged view table keeps `reload` flat. |
| `antares_view_builds_total` | Narrowed view handles a surface built (T-5896): one per view on its first request, again after `ANTARES_VIEW_IDLE` without one or after the view changed. Concurrent first requests share one build. A view served steadily keeps this flat. |
| `antares_notification_task_panics_total` | Delivery tasks that panicked (a bug, never expected). |
| `antares_change_lag_seconds` | Age of the change a notifier is handling. |
| `antares_policy_failures_total` | Decisions the policy seam had to make itself because the engine did not, by reason (`panic`, `timeout`). Always zero under the built-in `allow-all` engine. |
| `antares_pg_transaction_begin_seconds` | `postgres`/`timescale`: time to obtain a pooled connection and open a transaction — the pool wait plus one BEGIN round trip. |
| `antares_pg_pool_timeouts_total` | `postgres`/`timescale`: acquire timeouts, each one a request answered 503 with `Retry-After`. |

Every `_seconds` metric is a true histogram, bucketed at 5 ms, 10 ms, 25 ms,
50 ms, 100 ms, 250 ms, 500 ms, 1 s, 2.5 s, 5 s, 10 s, 30 s and 60 s. The
bounds are what `histogram_quantile()` can resolve, and the top ones exist
because service time reaches tens of seconds once the accept path saturates.

## Tenants

### GET /q/tenants

Answers the names, sorted, and nothing else:

```text
GET /q/tenants
["default","acme","zoo"]
```

Names only on purpose. A deployment runs up to 10 000 tenants (ADR-0001), and
a list carrying per-kind counts would cost a count per kind per tenant on one
request. `200` always; the route takes no parameters. Pick a name, then read
its detail:

The list is the customer accounts. The broker mints tenants of its own — one
per Snapshot, plus the two indexes — and none of them appears here or is
addressable through the routes below.

### GET /q/tenants/{tenant}

```text
GET /q/tenants/acme
{"tenant":"acme","counts":{"entities":0,"subscriptions":0,"csourceSubscriptions":0,
  "registrations":0,"snapshots":0,"entityMaps":0,"distSubs":0,"attrInstances":0},
 "bytes":{"entities":0,"attrInstances":0,"byType":{}},
 "estimated":false,"measuredAt":"2026-10-29T18:50:00.000Z"}
```

`200`, `404` for a tenant that does not exist, `400` for a name outside the
tenant grammar or one of the broker's internal names. `createdAt` is present
on Postgres, where the `tenants` table records it. The default tenant always
exists (5.5.10) and is always readable, even when empty.

`attrInstances` and `bytes` are the tenant's usage (T-2890, T-6085): the
history's attribute instances, and the stored bytes of the current Entities
(by first type in `byType`) and of the history; their sum is the tenant's
total. The history is counted and sized in one pass under a 1 s statement
bound. A longer history takes both numbers from the planner's statistics
(each history partition's row count and size times the tenant's share of its
`tenant_id` values) and the answer carries `"estimated": true`; ANALYZE
keeps these statistics, so a partition autovacuum has not analyzed yet
counts as empty. `measuredAt` is when the numbers were taken: the broker
keeps them 5 minutes per tenant, 1 minute after a failed measurement. When
neither the count nor the estimate answers, `attrInstances`, `bytes`,
`estimated` and `measuredAt` are left out and the other counts still answer.

### GET /q/tenants/{tenant}/types/{type}/attributes

```text
GET /q/tenants/acme/types/Room/attributes
{"tenant":"acme","type":"Room","attributes":["humidity","temperature"],"partial":false}
```

The Attribute names the tenant's Entities of one type hold, compacted under
the core @context; never a value. A plan reads them before a write that
replaces whole Entities: the view guard refuses such a write while a stored
Entity holds a name the view does not admit. `type` is a core-context term or
an expanded IRI, percent-encoded. `partial` is true when the fold stopped at
the scan ceiling. `200` with an empty list for a tenant or type with no
Entity, `400` for a name outside the tenant grammar or one of the broker's
internal names.

### DELETE /q/tenants/{tenant}

Purges the tenant: `204` when done, `404` for
an unknown tenant (`{"title":"ResourceNotFound","detail":"tenant nope"}`),
`409` while a distributed subscription of it still holds a copy at a
Context Source, `400` for a name outside the tenant grammar. The default
tenant is emptied and keeps existing. The path names the tenant; an
`NGSILD-Tenant` header is ignored. Background in
[operations](operations.md#tenants).


### Moving a tenant to another broker

ADR-0030. Hold the
writers first: the export is not a point-in-time snapshot, and the digest
proves the copy.

#### GET /q/tenants/{tenant}/digest

Answers `{"tenant","entities","attrInstances","sha256"}`. `sha256` is the
sum modulo 2²⁵⁶ of `sha256(id ‖ 0x00 ‖ modifiedAt)` over the Entities, so
two brokers agree whatever order they page in.

#### GET /q/tenants/{tenant}/export

Streams `application/x-ndjson`: a header `{"antaresExport":1,"tenant":…}`,
one `{"kind":"entity","doc":…}` per Entity, then one
`{"kind":"temporal","doc":…}` per history document, each as stored, and a
trailer `{"end":true,"records":n}`. A stream that stops without the trailer
was cut. A tenant holding Subscriptions, Registrations, Registration
Subscriptions, Snapshots or distributed subscriptions answers `409` with
their counts: v1 moves data only.

#### POST /q/tenants/{tenant}/import

Takes that stream into a tenant that holds nothing: no Entity, history,
Subscription, registration, Snapshot, EntityMap, distributed subscription
or hosted `@context` (`409` naming each count otherwise). The header names
the same tenant, entities come before history, a line is at most 16 MiB.
Any refused line, or a missing trailer, answers `400` naming the line; the
import then deletes the Entities and history it wrote, by id, and nothing
else. `204` when every record is in.

## Dead letters

A letter carries the subscription id, tenant, endpoint, headers, body,
attempt count, first and last error and timestamps. What produces one and
why an egress refusal never does:
[notification delivery](operations.md#notification-delivery).

### GET /q/dead-letters

`?tenant=&subscription=&limit=` — letters of one tenant (the default tenant
when `tenant` is absent), newest first, `limit` 100 by default; `400` for a
`limit` that is not a positive integer or a tenant outside the grammar.
Endpoint userinfo, `receiverInfo`, `notifierInfo` and the rendered `headers`
of an older letter are shown blanked; the stored letter keeps them so a
replay still authenticates.

### POST /q/dead-letters/{id}/replay

`?tenant=` — one attempt through the same binding under the egress policy of
the moment: `204` and the letter is gone, `502` with the failure text and the
letter kept, `404` when the tenant holds no such letter.

### DELETE /q/dead-letters/{id}

`?tenant=` — `204`, or `404` when the tenant holds no such letter.

## Views

Only in a broker built with the `views` feature (ADR-0023 rule 10); a
vanilla broker answers `404` here. A view is configuration: these routes
validate it, store it under the reserved tenant `antares-views` and reload
the pod's view table before they answer; every other pod reloads within
`ANTARES_VIEWS_RELOAD_MS` (default 5000). `/q/health` lists each view
(`loaded` with its tenant, operation count and loader notes, or `invalid`
with the reason) under `views` to a caller with the admin bearer (or to
everyone when no admin token is configured); an open caller gets
`{"count": n}` there, under `schemas` and under `policy.documents`, since
the reports name Tenants (F-ANT2-005). It lists each surface that serves through views
as `open` or `closed` under `surfaces`: no view loaded, every route of such
a surface answers `404`. `viewsRevision` is sixteen hex digits, a digest of
the stored view documents the table was loaded from: every pod over one
store reports the same value once it has reloaded, so two values mean two
pods serving two predicates.

```json
{
  "id": "cars", "tenant": "city",
  "@context": {"a": "https://a.example/"},
  "selector": ["Car"],
  "q": "speed>0", "scopeQ": "/city/#",
  "geoQ": {"georel": "within", "geometry": "Polygon", "coordinates": [[[0,0],[1,0],[1,1],[0,0]]]},
  "rules": [{"types": ["Car"], "q": "status==\"moving\"", "attributes": ["speed", "a:name"]}],
  "operations": ["query_entities", "retrieve_entity"]
}
```

`id` is `[a-z0-9-]{1,63}`; `tenant` is a client tenant; `selector`,
`rules[].types` and `rules[].attributes` expand through the view's own
`@context` (inline, over the core context); every `q` is read against the
core context; `operations` are row ids of `docs/analysis/operation-matrix.yaml`.
No member has a default that serves more: an empty selector, rule list or
operation list is refused. Bounds: 64 KiB per document, 64 rules, 256 names
per list, 4096 bytes per `q` or `scopeQ`.

A view of another model also carries `mapping`, the version 2 mapping IR
([views](views.md#5a-a-view-of-another-model)). The loader refuses an IR of
another version, a slot member its kind does not define, a selector other
than the IR's `sourceClass`, and any operation that is not a read. It loads
the view with a note for each slot whose source Attribute no rule shows:
such a slot is never served and no filter, sort or geo-query reads it.

### GET /q/views

The table, as `/q/health` lists it.

### GET /q/views/{id}

The stored document, or `404`.

### PUT /q/views/{id}

`201` created, `204` replaced; `400` with the first reason the view cannot
be served (nothing is stored), `413` over 64 KiB.

### DELETE /q/views/{id}

`204`, or `404` when there is no such view.

### POST /q/views/{id}/preview

What a draft would serve, before it is stored (the Portal's view writer).
The body is the candidate view document, checked as a PUT checks it; the
request's query string is a Query Entities query (5.7.2), run in the
draft's tenant through its narrowing under the subject headers of this
request (`ANTARES_POLICY_SUBJECT_HEADERS`): the admin caller names whose
reading it previews (the platform's Portal: a member's `x-jc-*`, or
`x-jc-roles: public` for the anonymous reading), and a request naming nobody
is the cluster's own caller, as on every other path (T-3813). `limit` is at most 100
(default 20) and any `count` is replaced by `count=true`. The answer is
`{"count": 2, "entities": [...]}`, the count of the whole narrowed set and
one page. Nothing is stored and no table reloads. `400` with the loader's
reason for a draft that cannot be served or a `limit` over 100, `413` over
64 KiB; a refusal of the query itself (no `type`, `attrs`, `q` or `georel`,
an unknown tenant, the engine's denial) comes back as the API gave it. No
view root reaches this route: a view serves only the operations of the
matrix.

## Export grants

Only in a broker built with the `internal-csr` feature and started with
`ANTARES_SELF_URLS` set (ADR-0025 rule 5): a Tenant's consent to another
Tenant's registrations reading it in process. See
[Internal context sources](internal-context-sources.md) for what a grant
allows; bound: 4 KiB per document.

### GET /q/grants

Every stored grant.

### GET /q/grants/{target}/{source}

The grant `target` holds for `source`, or `404`.

### PUT /q/grants/{target}/{source}

`{"target", "source", "access"?, "view"?}`: `201` created, `204` replaced;
`400` when the document is no grant, names a Tenant no client can, or names
another pair than the path (nothing is stored).

### DELETE /q/grants/{target}/{source}

`204`, or `404` when there is no such grant. The next forward that needed it
is not made.

## MCP write decisions

Only in a broker built with the `mcp` feature and started with
`ANTARES_MCP_CONFIRM_URL` set (T-5816): the deployment's decision page reads
the question a write tool asked and records the person's decision on it. See
[MCP](mcp.md) section 3. Questions live ten minutes in the process that
asked them; bound: 4 KiB per decision body.

### GET /q/mcp-confirmations/{id}

`{"id", "view", "tool", "message", "subject", "expiresInSeconds",
"decision"}`: the question the client was handed, the subject headers of
the call that asked (`{"x-jc-subject": ["…"]}`), and `decision` `null`,
`"accept"` or `"decline"`; `404` for an unknown, answered or expired id.

### PUT /q/mcp-confirmations/{id}

`{"decision": "accept" | "decline", "subject": {"<header>": "<value>"}}`:
the page names the person who decided by the subject headers it vouches
for, and each must be one of the asking call's. `200` with the question;
`404` when the id is unknown, answered or expired, or the person is not the
one who asked (the same answer, so a page learns nothing of another
person's questions); `400` for any other body. A second decision before
the client's retry replaces the first.

## Policy rule documents

Only in a broker built with the `views` feature and started with a policy
engine that reads rule documents (ADR-0020 amendment); with `allow-all`, or
an engine that reads none, every route here answers `404`. A rule document
belongs to one client tenant and is opaque to the broker: the engine owns
its schema. The broker bounds it (1 MiB, 32 levels of nesting, 131072 JSON
values), hands it to the engine, and stores it under the reserved tenant
`antares-policy` with a revision it counts. The table reloads with the view
table: the pod that took a write before it answers, every other pod within
`ANTARES_VIEWS_RELOAD_MS`. A stored revision the engine refuses on reload
(written by another version, or by hand) leaves the previous revision
deciding. `/q/health` lists the documents under `policy.documents` by
tenant and revision only; neither a document nor the engine's reason is
logged.

```json
{"loaded": [{"tenant": "city", "revision": 3}], "refused": []}
```

### GET /q/policy

The table, as `/q/health` lists it.

### GET /q/policy/{tenant}

`{"tenant": "city", "revision": 3, "document": {...}}`, or `404`; `400`
for a name that is not a client tenant.

### PUT /q/policy/{tenant}

The body is the document. `201` created, `204` replaced; `400` for no JSON,
`null`, a depth or value count over its bound, or the engine's reason for
refusing it (nothing is stored, the previous revision keeps deciding);
`413` over 1 MiB.

### DELETE /q/policy/{tenant}

`204`, and the engine drops the tenant's document; `404` when there is
none.

## Schemas

Only in a broker built with the `views` feature (T-3628). Each Tenant has at
most one `LinkML` schema, in its JSON serialisation, resolved (imports and
`is_a` folded): the model a surface such as OData or LinkML reads for value
types, relationship targets and kinds (`docs/analysis/odata-linkml-mapping.md`
3.1). The broker checks only its size and that it is JSON; the surface parses
it, so a schema it cannot read closes that surface for the Tenant's views and
the surface says so in `/q/health`. The document is stored beside the views
under `antares-views` and reloaded with them; `/q/health` lists each Tenant's
`bytes` and `revision` under `schemas`, never the document.

Every NGSI-LD write to the Tenant, on its root or through a view, and so
every OData and OGC API Features write, is held to the schema (T-5912):
each Attribute a class of the Entity lists keeps to that slot's NGSI-LD
kind, range (built-in type, enum, class), `multivalued` and `unece:` unit,
and a create or replace carries every `required` slot. A batch refuses the
breaking member alone. `ANTARES_SCHEMA_WRITES` ([configuration](configuration.md))
picks `enforce` (400 naming the attribute and the rule, never the value),
`report` (the default: logged, written) or `off`. The subset holds no
`pattern`, minimum, maximum or closed class, so none is checked; an
Attribute no class lists, an Entity of no class of the schema, a fragment
of a history-only Entity and a batch update member without `type` pass;
a Relationship's target type is not read.

The path names the Tenant, and only one a client can name; nothing in the
body does.

### GET /q/schemas/{tenant}

The schema as written, `application/json`, or `404`.

### PUT /q/schemas/{tenant}

`201` created, `204` replaced. `413` over `ANTARES_SCHEMA_MAX_BYTES`, `400`
when the body is not JSON or the path names no client tenant; nothing is
stored. `503` while `ANTARES_SCHEMA_MAX_BYTES` is unset: the operator names
the bound before any schema is taken.

### DELETE /q/schemas/{tenant}

`204`, or `404` when the Tenant has no schema.

## Retention

Only in a broker built with the `views` feature (T-3900). Each Tenant has at
most one retention document: how long it keeps each Entity type's current
state and history. A type without a rule, and a Tenant without a document,
lose nothing to their document; their history falls to the levels below.
The document is stored beside the views under `antares-views` and reloaded
with them.

```json
{"rules": [
  {"type": "https://example.org/Vehicle", "current": "PT5M", "history": "PT8H"},
  {"type": "https://example.org/Stop"},
  {"type": "https://example.org/School", "history": "off"},
  {"type": "https://example.org/Archive", "history": "forever"},
  {"type": "*", "history": "P90D"}
]}
```

- `type` is the expanded type IRI, as the store holds it; one rule per type,
  at most 256 rules.
- `current`: an Entity of that type that carries no `expiresAt` of its own
  and has not been modified for that long is deleted. An `expiresAt` the
  writer sent wins (4.22).
- `history`: the type's attribute instances whose `observedAt` (else
  `modifiedAt`, `deletedAt`, `createdAt`) is older than that are deleted.
- `history: "off"` (T-4275): the type keeps its current state only. The
  write path records no attribute instance for an Entity whose types include
  it, and the sweep deletes the instances it already holds, in the same
  batches, so a temporal read of the type answers an empty history once they
  are gone. Current state, subscriptions and notifications are unchanged. A
  write through the temporal API (`POST /temporal/entities…`) is not gated,
  and the next sweep removes it like the rest. A type with no `history`
  keeps its whole history.
- `history: "forever"` (DM-97): the type keeps all of its history, and
  neither the `*` rule nor the platform default reaches it.
- `type: "*"` (DM-97): the Tenant's default for every type whose rule says
  nothing of `history` (or that has no rule), a `history` only; `current` is
  refused there.
- A duration is ISO 8601 (`PT5M`, `PT8H`, `P30D`, `P1DT12H`) or short
  (`90s`, `5m`, `8h`, `14d`), from one minute to ten years.
- `record` (T-4665, platform ADR-N-054 2.2): `every` (the default) writes a
  history row for each new `observedAt`, even with the same value;
  `onChange` writes one when the value changed, with `heartbeat` (a
  duration, required) the longest gap between two rows. `deadband` (a
  number) holds back a numeric move no larger than it; `minDistance` (in
  metres) a GeoProperty move no longer than it. A correction of the same
  instant is always recorded. `observed` (T-5056) writes one only when the
  instance carries an `observedAt` newer than the one it replaces, so a
  value re-sent or changed under its old `observedAt`, or sent with none,
  adds no row; `never` writes none, for a bookkeeping Attribute such as
  `dateModified`. Neither takes settings.
- Under `ANTARES_TEMPORAL_RECORD=ruled` (T-5056) history is opt-in: only an
  Entity whose own `historyRetention`, whose type's rule or whose Tenant's
  `*` rule gives a duration or `forever` records rows, `observed` unless its
  rule names another `record`; every other type keeps its current state only, and its temporal Entity answers with no
  Attribute instead of a 404.
- `attributes`: per expanded Attribute name, its own `record`, `heartbeat`,
  `deadband` and `minDistance`, overriding the type's; `{"record": "every"}`
  keeps a count per interval whole under an `onChange` type. `record` and
  `attributes` are refused on the `*` rule.

```json
{"rules": [{"type": "https://example.org/ParkingSpot", "record": "onChange",
  "heartbeat": "PT1H", "deadband": 0.5,
  "attributes": {"https://example.org/count": {"record": "every"}}}]}
```

History retention has four levels and the most specific wins (DM-97):

1. The Entity's own Property `historyRetention`
   (`https://uri.etsi.org/ngsi-ld/default-context/historyRetention`), a
   duration or `off`. `off` records no attribute instance from its next
   write on; a duration holds even under a type whose rule is `off`. Any
   other value (`forever`, a typo) is ignored and logged once a sweep, so
   the type's rule applies.
2. The rule of one of its types in its Tenant's document.
3. The Tenant's `*` rule.
4. The platform default, `ANTARES_RETENTION_HISTORY_DEFAULT`
   ([Configuration](configuration.md)), in every Tenant with no `*` rule.

An Entity removed by its own `expiresAt` (4.22) takes its history with it.
Weekly history partitions below the pre-created window that the sweeps
emptied are dropped by the maintenance job, so their disk goes back.
Before its row deletes, each sweep drops a past weekly partition whole when
every Tenant with a row in it has a floor at or after the partition's end
(T-4906). A Tenant's floor is the oldest cut of all its levels. A Tenant
gets one only when its `*` rule or the platform default covers the rest of
its types, no type rule of it says `forever`, and every Entity with its own
`historyRetention` was read. One row of a Tenant without a floor, or with a
later one, keeps the partition, and the row sweep then deletes inside it.

Every pod sweeps once a minute, each rule in statements of at most 1000 rows
and at most 50 statements a sweep, logs per Tenant and type the count it
deleted, never ids or values, and counts it in
`antares_retention_deleted_total{tenant,type,kind}` (`kind` is `entity` or
`history`; `type` is `*` for the Tenant's default, `@entity` for what
Entities' own retention deleted and `@platform` for the platform default).
A sweep spends at most 15 s deleting and one statement at most 5 s. The
rules that waited longest run first, then the shortest retention. A rule
whose statement fails (the log line `retention sweep failed`) runs again 2,
4, 8 and at most 30 sweeps later, with half the batch per failure (at least
100 rows), and each clean sweep doubles its batch back. A rule that holds
a sweep for a second or more logs `retention sweep step slow` with its
batch, count and milliseconds. The history of a type (or of the Tenant's
other types) is deleted Entity by Entity, in id order and chunks of a tenth
of the batch (10 to 100 Entities): a step reads the chunk's own history
index range, never a join of the type to the whole history, goes on to the
next chunk for up to a second while the ones before held nothing to delete,
and the next sweep continues where it stopped.
`ANTARES_TEMPORAL_RETENTION_DAYS` stays the broker-wide horizon beside it.

### GET /q/retention/{tenant}

`200` with the document as written, `404` when the Tenant has none, `400`
for a name outside the tenant grammar.

### PUT /q/retention/{tenant}

`201` created, `204` replaced. `400` for a document that is not one (an
unknown member, a short type name, a type named twice, a duration out of
bounds), `413` over 64 KiB; nothing is stored.

### DELETE /q/retention/{tenant}

`204`, or `404` when the Tenant has no document.

### POST /q/retention/{tenant}/preview

The body is a retention document, checked as `PUT` checks it, and nothing is
stored or deleted. `200` with `{"counts": [{"type", "kind", "count"}]}`: per
type and kind (`entity`, `history`) what a sweep now would delete, at most
100 000 each, the platform default included and the Entities' own retention
left out; a type that would lose nothing is not listed. `400` and `413` as
`PUT`.

### GET /q/retention/{tenant}/deleted

`200` with `{"counts": [{"type", "kind", "count"}]}`: what this pod's sweeps
deleted of the Tenant since the pod started. Every pod sweeps the same rules,
so a pod's totals are its share; `antares_retention_deleted_total` sums them.

## Erasure

Only in a broker built with the `views` feature (T-4665, platform ADR-N-054
2.6, GDPR Art. 17). The admin credential is the erasure scope: the Portal
holds it, finds the Entities that name a data subject with the subject's
`q` over current state and history, and erases them here once the space's
Policy granted its caller `personalData`.

### POST /q/erasures/{tenant}

```json
{"entities": ["urn:ngsi-ld:BikeTrip:t1"],
 "attrs": ["https://example.org/rider"],
 "subject": "5e884898da280471",
 "actor": "steward-1", "reason": "Art. 17 request 2026-10-18"}
```

`entities` names 1 to 1000 Entity ids; `attrs` (expanded names) narrows the
erasure to those Attributes, and without it the whole Entity goes.
`subject` is the subject's keyed hash, for the log only: a value that looks
like anything else is refused. `actor` and `reason` are required. The
broker deletes the Attributes (or Entities) from the current state, every
history row and every superseded version of them, and the dead letters that
name the Entities, then stores one record beside the retention documents:
`201` with `{"id", "operation": "erase", "tenant", "entities", "attrs",
"subject", "actor", "reason", "at", "counts": {"entities", "history",
"deadLetters"}}`. The record holds no value. `400` for a request that is
not one, nothing erased.

Every pod applies every record again when it starts, so data a restore
brought back is erased before the pod serves. A re-applied erasure keeps
what was written after its `at`: an Entity created later, an Attribute
stamped later, a history row observed later.

### GET /q/erasures/{tenant}

`200` with the Tenant's erasure records, oldest first.

## The peer-facing wire

### POST /ex/v1/remote-notify

Where a Context Source posts a notification for a distributed subscription
this broker created (5.8.1.4). It is the one non-standard route that cannot
be firewalled off with `/q/`: every Context Source a subscription copy was
forwarded to has to reach it, and `ANTARES_PUBLIC_URL` is what that copy
advertises. It sits outside the `/ngsi-ld` prefix ETSI owns, and `v1`
versions the broker-to-broker wire independently of the NGSI-LD API version
(ADR-0019).

The body is an NGSI-LD Notification. Its `subscriptionId` is the routing
key, and it is a broker-generated UUID rather than the subscriber's own
Subscription id, so a Context Source learns nothing about the subscriber and
cannot address any other subscription. That key resolves through the stored
mapping to the tenant and the local subscription; the request's
`NGSILD-Tenant` header is not read on this route at all.

Who may post: whoever holds a key. Each key is a random UUID minted for one
subscription copy and sent only to the Context Source that copy went to, so
the key is the peer's credential for that one subscription. A caller without
one gets `404` before any store write or delivery (ADR-0019).

`200` when the notification was accepted — including when the mapping
resolves but nothing was left to deliver. `400` for a body that is not JSON,
a body without `subscriptionId`, or a `data` array carrying more Entities
than `ANTARES_MAX_BATCH_ITEMS` (the cap is applied before any store touch:
one notification drives one local retrieve and one federated fan-out per
Entity, so an uncapped array is an amplification lever). `404` when no
distributed subscription maps the key. Like `/q/`, this route carries the
body limit and the bounds wall itself — a peer-facing write path must not be
the one route where the documented caps do not apply.

## Deployment surfaces under /x

`/x`, and any path below it, belongs to the deployment. A surface registered
there ([extending](extending.md#api-surfaces)) mounts its own routes and
documents them itself; the broker only guarantees the ground rules. A prefix
outside `/q`, `/x` and below-`/x` is refused at startup, as is one that
overlaps a surface already mounted, so a deployment route can never shadow a
spec resource or race another surface for a path. `ANTARES_API_SURFACES`
names which surfaces are mounted, and `/q/health` reports each one's prefix
and route count under `surfaces`.

The shipped binary mounts `admin` and nothing else. The reference plugin
(`examples/plugin-example`, off by default) mounts `/x/example` and is the
worked example of both a surface and a façade.
