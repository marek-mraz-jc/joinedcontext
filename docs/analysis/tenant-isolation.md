# Tenant isolation: every store path takes the validated Tenant

Sources: Antares-NGSI-LD-Context-Broker `3232418` (origin/dev, branch `agent/worker-2/T-3914`), code graph built at that commit (`cg build --fg`, 14 650 symbols, 83 222 calls). Task T-3914. The queries are the two T-3914 statements of `dev/security.cypher`; the allowlist rows with their reasons are in `ci/cg-allowlist.txt`; `dev/cg-invariants.py` runs both.

## 1. The property

A request names its Tenant once, in `NGSILD-Tenant`, and `antares_api::negotiate::tenant_from` is the one reader (invariant `identity-headers`). From there the `TenantId` travels as a parameter to the store driver (`antares_store::CurrentStateDriver`, `TemporalDriver` and their `*Ext` traits), and each driver keeps rows of different Tenants apart: the memory store keys its maps by Tenant, the file store (redb) keys rows by Tenant and id, Postgres binds `tenant_id = $1` and sets `antares.tenant` for row-level security. Three checks hold the chain:

| Link | Check | Kind |
|---|---|---|
| a store method takes a Tenant | `store-method-without-tenant` | cg invariant, exact |
| a route hands the store the header's Tenant | `tenant-not-from-header` | cg invariant, approximate (section 3) |
| the driver honours it | `antares_store::contract::run_tenant_isolation_contract` | integration test, memory, file and Postgres |

## 2. Store methods without a Tenant

`store-method-without-tenant` returns 17 methods (the task counted 30 rows: each method appears twice, once by `HAS_METHOD` and once by `CONTAINS`; the statement returns `DISTINCT`). The `*Ext` traits (`mutate`, `batch_mutate`) take a Tenant everywhere. None of the 17 is a defect.

| Trait | Method | Classification | Reason |
|---|---|---|---|
| CurrentStateDriver | `close` | tenant-free by design | lifecycle: closes the pool at shutdown |
| CurrentStateDriver | `commit_queue` | tenant-free by design | metric: the group-commit queue depth |
| CurrentStateDriver | `outbox_ack` | cross-Tenant by design | acks the `seq` values `outbox_peek` handed out; Postgres runs it under the service role; `outbox_retain` and `outbox_event` take the Tenant |
| CurrentStateDriver | `outbox_peek` | cross-Tenant by design | the outbox relay drains every Tenant; each row is `(seq, tenant, event)` and is dispatched under its own Tenant |
| CurrentStateDriver | `history_overrides` | cross-Tenant by design | DM-97 retention loop: Entities with a `historyRetention` Property across Tenants, each row with its Tenant (Postgres: service role); no route reaches it (T-5012) |
| CurrentStateDriver | `ping` | tenant-free by design | readiness probe |
| CurrentStateDriver | `set_change_hook` | tenant-free by design | installs the change feed at boot; the hook receives `(tenant, before, after)` |
| CurrentStateDriver | `set_outbox` | tenant-free by design | boot switch |
| CurrentStateDriver | `subscription_tenants` | cross-Tenant by design | the inventory the interval sweep and the mirror hydration walk; names only, each Tenant is then read with its own `TenantId` |
| CurrentStateDriver | `sweep_expired` | cross-Tenant by design | 4.22 reap of expired rows (Postgres: service role, `maintenance.rs`) |
| CurrentStateDriver | `tenant_ids` | cross-Tenant by design | `/q/tenants`, behind the admin secret, names only |
| CurrentStateDriver | `version_info` | tenant-free by design | `/q/health` versions |
| TemporalDriver | `close` | tenant-free by design | lifecycle |
| TemporalDriver | `drop_retained_history` | Tenant inside the argument | T-4906 retention by partition: `floors` are `(tenant, cut)` pairs and a slice holding a row of another Tenant stays; retention loop only (T-5012) |
| TemporalDriver | `event_list` | Tenant inside the argument | every `TemporalEvent` carries `tenant`; the default fold groups consecutive events by `(tenant, entity_id)`; the isolation contract drains one event as `a` and checks `b`'s history |
| TemporalDriver | `ping` | tenant-free by design | readiness probe |
| TemporalDriver | `q_pushdown_exact` | tenant-free by design | a capability question about a `q` expression, reads no row |
| TemporalDriver | `supported` | tenant-free by design | capability flag |
| TemporalDriver | `version_info` | tenant-free by design | `/q/health` versions |

A new driver method without a Tenant turns the invariant red until it is classified here and allowlisted.

## 3. Routes and the Tenant they pass

`tenant-not-from-header` takes every HTTP route (`PROVIDES` an `http` Channel, 79 routes) that reaches a Tenant-taking store method within eight `CALLS|DISPATCHES` hops (73 routes) and reports the ones that do not call `negotiate::tenant_from` within four. Six routes do, and each takes its Tenant from somewhere other than a client header:

| Route | Handler | Where the Tenant comes from |
|---|---|---|
| `GET /q/dead-letters` | `dead_letters_list` | admin secret; `?tenant=` through `admin_tenant` (`TenantId::new`, reserved names refused) |
| `DELETE /q/dead-letters/{}` | `dead_letter_delete` | the same |
| `POST /q/dead-letters/{}/replay` | `dead_letter_replay` | the same |
| `GET /q/tenants/{}` | `tenant_get` | admin secret; the path segment, `TenantId::new` |
| `DELETE /q/tenants/{}` | `tenant_purge` | the same |
| `POST /ex/v1/remote-notify` | `distsub::remote_notify` | the broker's own inbound index under the peer's random `subscriptionId` (ADR-0019), never the request |
| `ANY /x/views/{}/ngsi-ld/v1/{}` | `antares_views::serve` | the view's own Tenant: `ViewOps::run_unmapped` sets it, refuses another or a doubled one with 400, and the inner route reads it with `tenant_from` (`views_decision.rs`, T-5012) |

What cg cannot express: data flow. The statement proves that a route calls the one header reader, not that the value it returns is the one handed to the store, and a route that read the header and then built a second `TenantId` from the body would pass. The driver contract below and the per-route tests in `antares-api` back the property where the graph stops. Work the broker starts itself (the notification sweep, the outbox relay, the mirror hydration) reaches no HTTP route; it takes its Tenant from the rows it walks (`subscription_tenants`, the outbox row), as section 2 lists.

Two `identity-headers` rows came with the WebSocket surface and are allowlisted with their reasons: `antares_ws::upgrade` reads the root connection's Tenant for its per-Tenant cap and for notification routing (every operation on the socket still passes the NGSI-LD router and its `tenant_from`), and `antares_ws::hub::Forwarded::of` reads the Tenant the notifier stamped on its own `receiverInfo`. The `surface-calls-the-store` statement named `AppState::store`, a path the graph no longer has (the fields live on `state::Shared`); it now names `Shared::store` and `Shared::temporal`.

## 4. The SQL

Every SQL string literal of `crates/antares-sql/src/store` (test modules excluded) that reads or writes a Tenant-bearing table: 71 statements, 58 with `tenant` in the text. The 13 without:

| Statement | Why no literal Tenant predicate |
|---|---|
| `pg/entity.rs:224`, `:235`, `:685` (`FROM entities WHERE {wheres}`) | `wheres` starts as `["tenant_id = $1", …]` (`entity.rs:489`) |
| `pg/temporal.rs:745`, `:771` (`FROM temporal_entities m WHERE {where_sql}`) | `where_sql` starts as `["m.tenant_id = $1", …]` (`temporal.rs:577`) |
| `pg/outbox.rs:30` ack, `:34` reap | cross-Tenant drain under the service role (`set_service`) |
| `pg/outbox.rs:31` retain | under `set_tenant`: row-level security limits it to the Tenant |
| `pg/doc.rs:906` | `Cached` @context rows belong to no Tenant (ADR-0021) |
| `pg/maintenance.rs:308`, `:360`, `:402`, `:423` | retention, expiry and partition maintenance, cross-Tenant under the service role; the `attr_instances` ones are named in `tests/attr_instances_tenant_predicate.rs` |

Row-level security covers every Tenant-bearing table (`tests/rls_covers_every_table.rs`) except `attr_instances` in TimescaleDB mode, where `tests/attr_instances_tenant_predicate.rs` requires the predicate in every request-path statement.

## 5. The driver contract

`antares_store::contract::run_tenant_isolation_contract` gives two Tenants an Entity, a Subscription, a Registration and a temporal document under the same ids, and then, acting as `a`: reads (`get`, `list`, `list_page`, `list_slice`, `query_entities`, the Subscription list, `matching_registrations`, `get_temporal`, `query_temporal`, the temporal `list`), writes (`mutate`, `upsert`, `batch_create`, `batch_upsert`, `batch_mutate`, `record_delivery`, `delete_entity_if`, `batch_delete`, `delete`, temporal `event_list`, `mutate`, `delete`) and purges `a`. After each write `b`'s copies must be unchanged, and after the purge `b` must be whole.

| Driver | Test | Pairs |
|---|---|---|
| memory | `antares-sql/tests/contract.rs::the_memory_store_keeps_tenants_apart` | `iso_b`/`isoxb` both ways, `iso`/`isox`, `isonamed`/default |
| file (redb) | `…::the_file_store_keeps_tenants_apart` | the same |
| Postgres | `antares-sql/tests/pg.rs::the_postgres_store_keeps_tenants_apart` | the same shapes with a run suffix |

`_` is the one character of the Tenant grammar (`[A-Za-z0-9_-]{1,64}`) that is also a LIKE wildcard, so `iso_b` beside `isoxb` catches a `LIKE` where `=` belongs; `iso` beside `isox` catches a prefix match. Names with `'`, `%`, `;`, a space, a backslash or a quote never reach a store: `TenantId::new` refuses them (`…::a_tenant_name_with_sql_metacharacters_never_reaches_a_store`). A purge that matched by prefix turns the memory and file runs red (checked by hand on this branch: "temporal purge as iso changed isox's history").

## Related

- `docs/analysis/surfaces-threat-model.md`
- `dev/security.cypher`, `ci/cg-allowlist.txt`, `dev/cg-invariants.py`
- T-3587 (the invariants), T-3915 (further invariants)
