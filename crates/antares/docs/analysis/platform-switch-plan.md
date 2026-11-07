# Switching the platform to the broker's surfaces: view writer, policy engine addon, deployment, callers (T-3525)

Sources: Antares-NGSI-LD-Context-Broker `5466e22` (dev) and `8c56d49` (branch `agent/worker-2/T-3511`, `cg` graph at `8c56d498d6`), joinedcontext-platform `main`, joinedcontext-portal `main`, joinedcontext-deployment `main`, all read on 2026-10-11 (read only, nothing built or run). Inputs: `docs/analysis/gateway-to-broker-map.md` (T-3436, "the map"), ADR-0020, ADR-0023, ADR-0027, `docs/analysis/operation-matrix.yaml`, `docs/analysis/mcp-surface.md`, `docs/analysis/odata-linkml-mapping.md`, `tasks/GOAL-antares-surfaces.md` points 10 and 11. Platform paths are under `crates/context-gateway/` of joinedcontext-platform unless another crate or repository is named; deployment paths are joinedcontext-deployment.

## 1. Scope and summary

The map (section 7) lists what `context-gateway` deletes once the broker serves it. Two things were missing before a "switch the callers" task could be written: something that turns a Portal Endpoint into an ADR-0023 view, and the joinedcontext policy engine behind the seam. This page decides both, lists the deployment changes, and gives per surface the callers to switch with the test that proves each. Section 8 lists the tasks filed from it.

Decisions, each with its options in the section named:

- **Paths do not change for callers** (section 5.1). The gateway keeps `/api/endpoint/{slug}/...` and `/cs/{space}/...`, verifies the token as today (D13), and forwards the request path-preserving to the broker's view root. The view id is the Endpoint slug, so no table is needed to find the view. This settles G16 and makes M3, M4, M6, M9, M15, M16, M18 to M21 path-neutral.
- **The Portal reconciler writes the views** (section 2.2), from the Endpoint manifests it already mirrors, through `/q/views` with the broker's admin token.
- **The engine addon lives in the Antares repository** under `addons/joinedcontext-policy/`, behind the off-by-default feature `jc-policy` (section 3.2). Its rules are the space's Policy manifests, compiled by the Portal and written through `/q` as one rule document per Tenant (section 3.3). That, and a deployment image that carries an engine, need an ADR-0020 amendment, filed as its own task.
- **The deployed image is a second image** built from the same Dockerfile with a features build argument (section 4.1). The vanilla image and its gates stay as they are; the deployed one makes no conformance claim, as ADR-0020 says of any engine other than `AllowAll` (`docs/adr/ADR-0020-policy-seam.md:201-203`).

## 2. Endpoint to view

### 2.1 Field by field

The Endpoint kind is `EndpointSpec` (joinedcontext-platform `crates/jc-core/src/kinds/endpoint.rs:344-410`); the view is `View` (`crates/antares-ops/src/views.rs:84-112`, `deny_unknown_fields`). An Endpoint has no types, attributes or operations of its own: types and attributes come from its `ModelProjection` (`kinds/model_projection.rs:89-100`) or, without one, from the Policies bound to it (`kinds/policy.rs:618-648`; with no `policyRef` every Policy of its space, `context-gateway/src/store.rs:345-348`).

| View member | From | Rule |
|---|---|---|
| `id` | `spec.slug` (`endpoint.rs:348`, `^[a-z2-7]{26,}$`, `:11-18`) | The slug fits the view id's `[a-z0-9-]{1,63}` (`views.rs:293-298`) for slugs of 26 to 63 characters; the generator mints 26 (`generate_slug`, portal `src/apps/reconciler.rs:219`). A longer hand-written slug is refused by the writer with the reason in the Endpoint's status. The `/cs/{space}` surface gets the view `cs-{segment}` (section 5.1). |
| `tenant` | the space's URN segment: `urnSegment`, else `{project}-{name}` (`kinds/space.rs:246`, `:276-281`) | The same value the gateway pins as `NGSILD-Tenant` today (`store.rs:337-339`, `resolver.rs:103`). |
| `selector` | projection `classes[].name`, else the union of the bound Policies' `information[].entities[].type`, else the space data model's classes | At least one, at most 256 (`views.rs:300-318`). An Endpoint whose sources name more than 256 types is refused by the writer (status message), not truncated. |
| `rules` | one rule per projection class: `types: [class]`, `attributes`: its `slots` less `projection.hiddenAttributes` (`endpoint.rs:378`, EP-61); without a projection, one rule per selected type with the union of the Policies' `propertyNames` and `relationshipNames` (empty = the whole Entity) less the hidden ones | At most 64 rules (`MAX_RULES`, `views.rs:58`). These are the widest the Endpoint ever shows; who sees how much of it is the engine's (section 3). |
| `q`, `scopeQ`, `geoQ` | `projection.filter.q`, `.scopeQ`, `.geoQ` (`model_projection.rs` `ProjectionFilter`) | Copied. `temporalQ` has no view member: an Endpoint with a projection `temporalQ` stays on the gateway until G6 (section 3.1) lands; the writer refuses it with that reason. |
| `operations` | union of the bound Policies' `operations` (`policy.rs:629`; `OperationRef` single or group, `:430`), each CIM 009 name mapped to the matrix row whose `cim009` clause it names (map section 2.2 has the name-to-clause table), groups expanded by Table 4.20-2 | An Endpoint serving `mcp` keeps the same list: the MCP tools follow the view's allowlist (`mcp-surface.md`). Operations outside the map's 2.2 table (registrations, `@context` management, snapshots, purge) never enter a view, which is what S16 asked for. |
| `@context` | none | The view's names are written as expanded IRIs, so no context is needed and K30 holds by construction (`ADR-0020:112-123`). |
| (schema) | the resolved LinkML JSON of the space's `dataModelRef` | Not a view member (T-3628): the writer PUTs it to `/q/schemas/{tenant}` once per Tenant, and each view narrows it by its rules, so a projection's classes and slots reach the surface as the view's `rules` (`docs/src/linkml.md` section 1). The broker needs `ANTARES_SCHEMA_MAX_BYTES` set (T-3642), or every schema write answers 503 and the LinkML surface stays 404 (section 5.4). |

Endpoint fields that do not reach the view, and where they stay:

| Field | Stays in | Why |
|---|---|---|
| `audience`, `allowedProjects`, `callerRole`, `roles` | gateway (admission, D13) and engine rule data (section 3) | identity and audience are token checks (B9); a role a subject holds is an input of the engine's rule match. |
| `enabledRepresentations`, `mcp` | the gateway's routing | a representation the Endpoint does not serve keeps answering the gateway's 404 (K34). |
| `rateLimits`, `creates`, `fileLimits`, `caching` | gateway (D13, K31, K35) | quotas and request transforms. |
| `publish`, `catalog` | gateway and Portal (D1) | registry records. |
| `viewMappingRef` | gateway, unchanged | G18: a view of another model is a transform, not a narrowing. Such an Endpoint keeps the gateway path until the G18 task exists; the writer skips it. |
| `policyRef` | engine rule document (section 3.3) | selects which Policies apply. |

### 2.2 Who writes the views

Options: (a) the Portal reconciler; (b) the gateway, which reads the same repository (`store.rs:108-160`); (c) `jcctl` at assembly time.

Decided (worker-2, owner may override): (a). The Portal already mirrors the Endpoint manifests from Gitea (`src/store.rs:39-51`), runs one leader (`src/reconciler/leader.rs`), reaches the broker on its API port (`src/reconciler/registrations.rs`, NetworkPolicy `components/portal/networkpolicies.yaml:65-72`) and reports per-object status. (b) would make the token validator also an admin writer of the broker, and every gateway replica a writer. (c) runs only when a repository is assembled, so a revoked Endpoint would wait for the next assembly.

The writer is a new reconciler module, `src/reconciler/views.rs`:

- on every pass, the desired set is the views of section 2.1 for every Endpoint whose `enabledRepresentations` hold `ngsi-ld` or `mcp`, plus one `cs-{segment}` view per Context Space;
- `GET /q/views`, `PUT /q/views/{id}` for each changed one, `DELETE /q/views/{id}` for each it owns that is gone. It owns the ids matching the slug pattern or `cs-`, so a view written by hand under another id is never deleted;
- refusals of the writer (section 2.1) and the broker's own 400 (`views.rs:591-625`) become the Endpoint's status;
- with `Authorization: Bearer` the broker's admin token, read from a Secret by `secretRef` (`ANTARES_ADMIN_TOKEN_FILE`, `crates/antares-api/src/admin_auth.rs:2-7`; at least 32 bytes, `docs/src/admin-api.md:13-25`).

`/q` protection, ADR-0020's "the admin surface is the gateway's to protect" (`ADR-0020:189-190`): no APISIX route reaches the broker (`tests/test_edge_attack_surface.py:206-212`, `:543-551` assert it), the broker admits only the gateway and the Portal (`components/context-broker/networkpolicies.yaml:43-55`), and today nothing else guards `/q`: no admin token is set (`values/broker/base-values.yaml.gotmpl:54-63`) and no mesh authorization is rendered for the broker (`charts/workload/templates/meshauthorization.yaml:8-9`). The deployment task sets the token (section 4.2) so the gateway, which reaches the broker's port too, cannot write views. The gateway only ever builds `/x/views/...` paths (section 5.1) and never forwards a client path under `/q`.

## 3. The joinedcontext policy engine addon

### 3.1 Rule kinds against today's seam

The map's section 3 was written against a five-member `Filter`. The seam has grown since; `Filter` now has `q`, `scope_q`, `geo_q`, `geo_also`, `omit`, `pick`, `rules`, `omit_by_type` and `restricted` (`crates/antares-api/src/policy.rs:236-275`), `Rule` has `types`, `q`, `scope_q`, `geo_q`, `geo_also`, `attributes` (`:282-295`), `FILTERABLE` holds 5.7.1 to 5.7.10, 5.14.4 and 5.14.5 (`:82-85`), a batch is decided member by member (`decide_each`, `:851`, T-3480) and a write by id carries the stored image (`Operation.stored`, `:136-140`, T-3483). The gaps of map section 3.1, as they stand:

| Gap | Rule kinds | State | How the engine answers |
|---|---|---|---|
| G1 types | K7, K17, K18, K20 | closed by `Filter.rules` (T-3528): an Entity no rule holds for is left out (`policy.rs:268`) | one `Rule` per granted type |
| G2 ids, `idPattern` | K8, K20 | **open**: neither `Filter` nor `Rule` names ids | reads: open (task filed, section 8); writes: `Deny` from `Operation.ids` |
| G3 per-policy (q ∧ scope) OR-ed | K10 | closed by `Filter.rules`: each rule is one branch with its own `q` and `scope_q`, and an Entity is served by the union of the rules that hold for it | one `Rule` per Policy and type |
| G4 slots per type | K17, K18 | closed (T-3528, `Rule.attributes`; `omit_by_type`) | `Rule.attributes` |
| G5 geo | K12, K17, K20 | closed (T-3484, T-3543: `geo_q`, `geo_also`, `Rule.geo_q`) | per-rule area |
| G6 temporal clamp | K13, K17 | **open** | task filed; until then an Endpoint whose Policies carry `temporalQ` stays on the gateway path |
| G7 "the answer is nothing" | K14, K16, K18 | partly: a `Filter` whose only rule names no type holds for nothing (`Rule.types`, `policy.rs:282-295`), so the answer is empty; the store is still asked | accepted as is: the count and the paging describe the empty set, and the map's timing concern (G7) is the same for an empty tenant |
| G8 dropped-types warning | K18 | **open** (no `warning` member) | O-4 decided (b) in the map; task filed |
| G9 batch division | K23 | closed (T-3480, `decide_each`) | per member |
| G10 discovery | K21 | closed (T-3479, `FILTERABLE`) | `Filter.rules` narrows `/types`, `/attributes`, and the LinkML surface (T-3511) |
| G11 per-entity notification selection | K25 | not checked here; T-3499 decides view ∧ subscription at match time | the switch task for 7.3 checks it with the engine attached |
| G12 grants on stored state | K24 | yes (T-3634): the engine answers `AllowIf(filter)` on a write by id and the store applies the write only if the stored Entity matches, inside its own read-modify-write | a conditioned write that would be forwarded is refused; a temporal write or a Batch Upsert under such a grant answers `Deny` (fail closed) |
| G13 preview | R11, M10 | **open** | task filed (preview of a draft view through `/q`) |
| G15 subject | K1, K2, K34, K38 | open in the deployment | section 4.2 |
| G17 relationship targets | K29 | the matrix names the linked-entity check per write row (`operation-matrix.yaml`, `linked_entity_checks`); whether the view write path enforces it for an engine `Filter` is not checked here | the 7.1 switch task proves it with a test per M7 caller |
| G18 view mapping | K26 | **open** | Endpoints with `viewMappingRef` stay on the gateway (section 2.1) |
| G19 units | K28 | STAYS (O-6) | gateway request transform |

K3 to K6 (operations, validity, prohibition first, default deny), K11 (write scope), K22 (write guard), K27 (declared types) are decisions on `Operation` alone (`clause`, `types`, `attrs`, `body`, `stored`) and need no seam change. K15 (restricted signal) is `Filter.restricted` with O-3 decided (b): the engine sets it only when the subject carries the opt-in (section 4.2 forwards it).

### 3.2 Where it lives and how the image carries it

Options: (a) a crate in the Antares repository outside `crates/`, behind an off-by-default `antares-broker` feature, as ADR-0020 requires of every engine (`ADR-0020:192-196`); (b) a crate in joinedcontext-platform that builds its own broker binary.

Decided (worker-2, owner may override): (a), `addons/joinedcontext-policy/` (crate `joinedcontext-policy`, engine name `joinedcontext`, feature `jc-policy`, shelved in `POLICY_SHELF` like `plugin-example`, `crates/antares-broker/src/main.rs:528-536`). (b) is not possible without a core change: the engine shelf is a compile-time list in the broker's `main.rs`, and no core crate may name an engine (`ADR-0020:192-196`), so an engine outside the repository cannot be shelved. `dev/check-no-addon.sh` bans `antares-policy-` names (`:22`); the crate name `joinedcontext-policy` and the path `addons/` are added to its forbidden list, so the vanilla image can never carry it.

ADR-0020 also says an engine is "built and run by `examples.yml` alone" and that "the shipped image, `ci`, `full`, `strict` and every ETSI cell run `AllowAll`" (`ADR-0020:192-196`). An image that carries the engine for one deployment (section 4.1) contradicts the first half, so the ADR-0020 amendment of section 3.3 also states it: the Antares release image and every conformance cell keep `AllowAll`; one deployment image per engine may be built from the same Dockerfile, is never published as `antares-broker`, and claims no conformance (`ADR-0020:201-203`).

The engine is only rules over `Operation` and `Subject`: no network call, no store access, its rule table in memory, swapped whole on reload. `ANTARES_POLICY_TIMEOUT_MS` (default 250 ms, `docs/src/configuration.md:56`) fails it closed.

### 3.3 Its data source (A2)

Options: (a) a rule document per Tenant written through `/q` and stored by the broker beside the view table (the T-3496 pattern, reload every `ANTARES_VIEWS_RELOAD_MS`); (b) a file mounted from a ConfigMap or Secret the Portal renders; (c) the gateway's repository checkout, as today.

Decided (worker-2, owner may override): (a). It is ADR-0014 rule 3 ("reloaded at runtime from the stores the broker already has", map A2), every pod sees the same revision, and a revocation reaches the broker within one reload (5 s default, `views.rs:41`), close to today's 1 s (`pdp/reaper.rs:27`, K39). (b) waits for the kubelet's ConfigMap sync, which is tens of seconds; (c) needs a repository inside the broker pod, which the deployment's vanilla test forbids (`tests/test_context_broker_vanilla.py:28-35`: no `REPO` or `GITSYNC` env).

What (a) needs that the seam lacks: an engine cannot mount a route (surfaces live under `/x/`, `crates/antares-api/src/surface.rs:36-43`) and has no hook to receive data. The amendment, as its own ADR task: a generic `/q/policy/{tenant}` document table in `antares-ops` (opaque JSON, bounded, `deny`-free because the engine owns its schema), handed to the engine through a new `PolicyEngine::load(tenant, &Value) -> Result<(), String>` with a default that refuses, so `AllowAll` and a broker without an engine answer `/q/policy` 404. A document the engine refuses keeps the previous one and is reported in `/q/health`, as a view that fails to load is.

The document is compiled by the Portal from the space's Policy manifests: for each Policy (`policy.rs:618-648`) its `effect`, `assignee`, `operations` as clauses, `validity`, and per `information` entry one `Rule` (expanded types, `propertyNames` and `relationshipNames` as attributes, `q`, `scopeQ`, `geoQ`), plus per Endpoint its `roles` and `callerRole` (`endpoint.rs:405-419`) and its `projection.hiddenAttributes`. The compiler is the second half of the view writer (same module, same pass). Data-space agreements (K38) are compiled into the same document once their compile path is known (map section 8: `odrl_compiler::compile` has no production caller).

## 4. Deployment

### 4.1 Image

The published broker image is vanilla by construction: the Dockerfile builds `cargo auditable build --release --locked -p antares-broker` with no `--features` (`Dockerfile:37`, default `mqtt`), `build-image.yml` runs `dev/check-no-addon.sh` first (`:51-52`) and `dev/check-vanilla-symbols.sh --audit-info` after (`:74-80`), and the deployment pins `ghcr.io/joinedcontext/antares-broker:0.1.2@sha256:44f78d69…` (`components/context-broker/images.yaml:10-12`). No `full` feature exists in `crates/antares-broker/Cargo.toml` yet, although ADR-0027 names one ("One binary, one image: `full` serves every surface", Consequences).

Decided (worker-2, owner may override): `ARG FEATURES=""` in the Dockerfile, passed as `--features "$FEATURES"` (empty = today's build, so the vanilla image is byte-for-byte the same build); a `full` feature (`views`, `mcp`, `linkml`, and `ws`, `odata` once they exist); a separate workflow `image-jc.yml` that builds `FEATURES=full,jc-policy` as `ghcr.io/<owner>/antares-broker-jc`, signs it like `full.yml:441`, and skips `check-no-addon.sh` only for itself. Its ETSI cells are those of the full binary (T-3501), run with `ANTARES_POLICY=allow-all`; with the engine on it claims nothing (`ADR-0020:201-203`).

### 4.2 Values, secrets, network

| Change | Where | Note |
|---|---|---|
| image repository, tag and digest → `antares-broker-jc` | `components/context-broker/images.yaml:10-12` | digest edited by hand today (no recipe; Renovate captures no digest, `renovate.json:58-68`); `scripts/validate-dev.py:47-49` checks signatures only under `ghcr.io/marek-mraz-jc/` and `ghcr.io/marek-mraz/`, so the image goes under one of them or the prefix list grows |
| `ANTARES_API_SURFACES=admin,views,mcp,linkml` | `values/broker/base-values.yaml.gotmpl:54-63` | `tests/test_context_broker_vanilla.py:28-35` allows `ANTARES_*`; the test's name and docstring say vanilla and are updated with it |
| `ANTARES_POLICY=joinedcontext`, `ANTARES_POLICY_SUBJECT_HEADERS=x-jc-subject,x-jc-groups,x-jc-roles,x-jc-client,x-jc-restricted,x-jc-did` | same | names decided here; the gateway sets them (next row); `x-jc-did` for a data-space participant (K38, T-3809) |
| gateway sets the verified subject headers and stops forwarding `Authorization` | platform `src/middleware/tenancy.rs:20-27` (strip list), `src/proxy.rs` (`copy_relayable`) | today no subject header is set and the caller's bearer token appears to reach the broker unchanged (`proxy.rs:28-37` drops only hop-by-hop headers; the MCP re-entry re-attaches it, `app.rs:644-645`); the broker ignores it, but a credential should end at its validator |
| `ANTARES_ADMIN_TOKEN_FILE` from a new Secret `context-broker-admin-token`, the same Secret mounted into the Portal for the view writer | `components/context-broker/secrets.yaml`, `components/portal/` | 32 bytes at least; never in values or Git |
| broker egress to notification targets | `components/context-broker/networkpolicies.yaml:35-42` (today only the gateway, R46) | `0.0.0.0/0` on 80/443 except the private ranges, as `policies/egress.rego:10` requires of the gateway; the broker's own egress check (ADR-0019) holds the rest. Lands with 7.3, not before |
| Prometheus to `/q/metrics` | broker ingress | found on the way: the ServiceMonitor scrapes `/q/metrics` (`components/monitoring/values/monitoring/base-values.yaml.gotmpl:26-35`) and no broker ingress admits it; with an admin token, `ANTARES_ADMIN_METRICS=open` keeps it scrapable |

No APISIX change: the gateway stays the only upstream of every data path (section 5.1), and no route may reach the broker (`tests/test_edge_attack_surface.py:543-551`). Port question of the map's unknowns, resolved: the broker Service maps 8080 to the container's 9090 (`values/broker/base-values.yaml.gotmpl:22-29`), so `JC_GATEWAY_BROKER_URL` (`:8080`, the Service) and the NetworkPolicy (`9090`, the pod port after DNAT) agree.

## 5. Per surface: callers, the test that proves each, then the deletion

### 5.1 How a request reaches the view

Options for G16: (a) the gateway keeps the platform paths and maps them to the view root; (b) callers move to `/x/views/{view}/...` through a new APISIX route; (c) the broker learns slugs.

Decided (worker-2, owner may override): (a). `/api/endpoint/{slug}/{rest}` → broker `/x/views/{slug}/{rest'}`, `/cs/{space}/{rest}` → `/x/views/cs-{segment}/{rest'}`, where `{rest'}` is `ngsi-ld/v1/...`, `mcp` or `linkml/...` (section 5.4). The gateway keeps D13 in front (token, audience, rate limits, quotas, tenancy), and it is the one path, which GOAL point 11 allows ("no dual path, fallback flag"): the switch task changes the upstream with no flag and no fallback, and the delete task that follows it removes the pipeline nothing calls any more. (b) would expose the broker at the edge, which the deployment's edge tests forbid (section 4.2), and would make every App and SDK caller change its URL (M3, M20). (c) puts the registry into the broker (G16 rejected it in the map).

The space surface `/cs/{space}` serves the canonical audience (SP-01, SP-03) with every Policy of the space; its view `cs-{segment}` selects the space model's classes, and the engine's document narrows it per subject like any Endpoint.

### 5.2 NGSI-LD endpoint view (map 7.1)

Switch: the gateway's NGSI-LD routes (R2, R3, R24, R31) forward through 5.1; `serve_ngsi_ld` and the PDP go in the delete task right after (map 7.1 (a)). Before it, every security test of map 7.1 (c) is carried over to Antares (the T-3444 test plan's rule: each becomes an Antares test first).

| Caller (map row) | Proving test, against a broker with views and the engine (the platform already starts one: `tests/antares_integration_tests.rs:21`, image `ghcr.io/marek-mraz/antares-broker:dev`, to become the `-jc` image) |
|---|---|
| M2 grid count | `sdk` vitest: a narrowed page carries `NGSILD-Results-Count` of the narrowed set and the grid shows the total; `transport.ts:17` comment updated |
| M3, M9 SDK and UI reads under `/api/endpoint/{slug}` | gateway integration: read, query, `join`, `pick` through the slug land on `/x/views/{slug}/ngsi-ld/v1` and answer the view's narrowing |
| M4, M6 `/cs/{space}` reads (UI, drift) | gateway integration: the space surface lands on `cs-{segment}`; portal `drift_tests` read through it |
| M7 writes (`EntityWriteCard`, `FormView`, `importRows`) | per caller: an out-of-view write answers 404, a refused operation 403 `urn:antares:error:AccessDenied` (O-8), and the UI shows both; units (K28) still filled by the gateway |
| M8 temporal chart | temporal read through the view; G6 open, so a Policy with `temporalQ` keeps its Endpoint on the gateway path until G6 lands (section 3.1) |
| M17 pipelines (`streams.rs:1266`, `:1992`, `pipeline_expiry.rs:279`) | Bento batch upsert through the slug: a refused member comes back in the 207 member by member (G9 closed) |
| M1 restricted signal | the engine sets `restricted` only when `x-jc-restricted` is forwarded from the caller's opt-in (O-3 (b)) |
| M22 Portal registration sync | `registrations.rs` keeps the plain `/ngsi-ld/v1` root with no subject header; the engine's rule for a request without subject on the plain root is decided in the engine task (section 8) |

Delete: map 7.1 (a) to (d); Done when its Cypher returns 0 rows.

### 5.3 MCP endpoint surface (map 7.2)

Switch: R5 and R25 forward to `/x/views/{slug}/mcp` and `/x/views/cs-{segment}/mcp`; the hub (R6) and named servers (R8) stay (D6) and forward each tool call there. Proving tests: M13, the assistant's chat (`crates/assistant/src/chat/mcp.rs:55`) and the Portal's MCP page (`ui/src/pages/mcp/mcp.ts:181`) list and call the same tool names against the broker surface (the names the matrix marks `exists` are the gateway's, `operation-matrix.yaml` `surfaces.mcp.status`); `mcp_hub_tests` and `mcp_server_tests` switch to a broker stub (map 7.2 (c)). The Keycloak resource indicator (`components/context-gateway/keycloak-clients.yaml:99`) keeps its value because the path does not change. Write tools need T-3514 first.

### 5.4 Schema surface (map 7.4)

Switch: R26, R28, R30, R32 forward `/api/endpoint/{slug}/schema/...` to `/x/views/{slug}/linkml/schema/...` (T-3511 keeps the platform's file layout and index format, `docs/src/linkml.md` section 3). Proving tests: M11, `EndpointDataView`, `AppGenerator`, `SchemaProjectionPanel`, the SDK's `client.ts:506` and the one-shot agent's `model.rs:526` read `schema/index.json` and one artifact through the gateway against the broker. `describedby` links (`src/middleware/response.rs:84`) then name the same gateway URLs, so `handlers::schema::Artifact` moves to a small type `response.rs` owns (map 7.4 (a)). The view writer sends `linkml` until T-3628 makes the schema per Tenant.

### 5.5 Notification egress (map 7.3)

Switch: subscriptions created through 5.1 are stored with `__subject` and delivered by the broker under `pre_notify` (B4, D9); the broker egress opens (section 4.2). Stored subscriptions whose endpoint points at R12 move by O-7: reconciler-owned ones are recreated by the Portal (`src/reconciler/subscriptions.rs:266`), the rest by a one-time migration task. Proving tests: M5, a reconciler-created subscription delivers to its target directly with the narrowed payload, and a revoked grant stops the next delivery (`notification_egress_tests.rs::a_revoked_grant_stops_the_subscription_it_allowed`, carried over). Delete: map 7.3 (a) to (d), `JC_GATEWAY_DELIVERY_KEY` and its Secret included.

### 5.6 Access documents (map 7.5)

The access document and the AuthZEN check render the engine's rules (D4), and the engine cannot serve a route (section 3.3). Decided (worker-2, owner may override): the addon crate also ships a surface, `/x/views/{view}/access` (GET, and POST for the AuthZEN check), shelved under `ANTARES_API_SURFACES=...,access` like `example` (`main.rs:470-482`), answering from the same rule table the engine decides with, so the two cannot drift (EP-60, `src/handlers/access_odrl.rs:6`). M12 callers (`src/agents/held.rs:132`, `EffectivePermissions.tsx:196`, `AccessPanel.tsx:64`) keep their gateway URLs through 5.1.

### 5.7 Translators that stay (map 7.6)

Files, dumps, OGC Features and SensorThings (D10) stay in the gateway and read `/x/views/{slug}/ngsi-ld/v1` with the forwarded subject instead of calling `Pdp::decide`. Proving tests: the decision assertions of map 7.6 (c) run against the broker. This lands after 5.2, since it reads what 5.2 switches.

## 6. Order

1. Seam: ADR-0020 amendment and `/q/policy` (3.3); open gaps G2, G6, G8, G12, G13 in parallel.
2. Antares: `full` feature, `FEATURES` build argument and `image-jc.yml` (4.1); engine addon and its access surface (3.2, 5.6).
3. Portal: view writer and rule compiler (2.2, 3.3).
4. Gateway: subject headers, `Authorization` no longer forwarded, path mapping (4.2, 5.1).
5. Deployment: image, env, admin token, Secret (4.2).
6. Per surface in the order 5.2, 5.4, 5.3, 5.6, 5.7, 5.5: carry the security tests over, switch, delete, `dev` smoke green. 5.5 goes last because it changes the broker's egress and stored subscriptions.

## 7. Unknowns

Resolved here: the 8080/9090 port difference (section 4.2); `scope` inside `q` (G3) no longer matters, because `Filter.rules` carries one `scope_q` per branch; the image question behind "the engine addon built into the deployed broker image" (section 4.1).

Still unknown, each named in the task that meets it:

- whether the broker's bounds (`crate::bounds`) are as tight as the gateway's caps (S17): 5.2's switch task compares them;
- whether the broker echoes `NGSILD-Tenant` (S22): the gateway's scrub stays (K37) either way;
- which UI code reads the gateway's `code` member (M7): 5.2's switch task;
- where data-space agreements are compiled in production (map section 8): K38 waits for it;
- whether the view write path applies the matrix's linked-entity checks under an engine `Filter` (G17): 5.2's switch task;

Stale citations in the map, found while reading and corrected there in this commit (the code moved since T-3436), and `docs/src/views.md`, which said an empty view table answers 503: the code answers 404 (`crates/antares-views/src/lib.rs:129-131`, `crates/antares-broker/tests/surfaces_fail_closed.rs::a_selected_surface_without_a_view_is_closed`), as ADR-0023 (`:55-56`) says. the addon rule is `ADR-0020:192-196` (map: `:187-191`), `RESTRICTED_HEADER` is `policy.rs:662` (map: `:232`), `ACCESS_DENIED_TYPE` is `policy.rs:976` (map: `:470`), `SUBJECT_HEADERS` is `policy.rs:984` (map: `:478`).

## 8. Tasks filed

Filed with `tasks/newid`, group `antares-surfaces`, batch `antares-surfaces-switch`, each id added to T-3446's `depends-on:`.

| Task | Repository | What | Section | Depends on |
|---|---|---|---|---|
| T-3630 | Antares | ADR-0020 amendment: `/q/policy/{tenant}`, `PolicyEngine::load`, deployment image per engine | 3.2, 3.3 | T-3496 |
| T-3631 | Antares | G2: ids and `idPattern` in `Filter` and `Rule` | 3.1 | T-3528 |
| T-3632 | Antares | G6: temporal narrowing | 3.1 | T-3528 |
| T-3633 | Antares | G8: `Filter.warning` as `NGSILD-Warning` | 3.1 | T-3528 |
| T-3634 | Antares | G12: write applied only if the stored Entity matches | 3.1 | T-3483 |
| T-3635 | Antares | G13: preview of a draft view through `/q` | 3.1 | T-3496 |
| T-3636 | Antares | `full` feature, `FEATURES` build argument, `image-jc.yml` | 4.1 | T-3630, T-3637 |
| T-3637 | Antares | engine addon `addons/joinedcontext-policy` | 3.2, 3.3 | T-3630 |
| T-3638 | Antares | addon surface `/x/views/{view}/access` | 5.6 | T-3637 |
| T-3639 | Antares | analysis: G18, views of another model | 3.1 | T-3497 |
| T-3640 | portal | view writer and rule compiler | 2.2, 3.3 | T-3630, T-3496 |
| T-3641 | platform | subject headers, no `Authorization` upstream | 4.2 | T-3637 |
| T-3642 | deployment | `-jc` image, env, admin token Secret, metrics | 4.2 | T-3636, T-3637, T-3640, T-3641 |
| T-3643, T-3644 | platform | switch, delete: NGSI-LD endpoint view (7.1) | 5.2 | T-3642, T-3497..T-3499; delete also T-3631..T-3635, T-3639 |
| T-3645, T-3646 | platform | switch, delete: schema surface (7.4) | 5.4 | T-3643, T-3511 |
| T-3647, T-3648 | platform | switch, delete: MCP endpoint surface (7.2) | 5.3 | T-3643, T-3513, T-3514 |
| T-3649, T-3650 | platform | switch, delete: access documents (7.5) | 5.6 | T-3643, T-3638 |
| T-3651, T-3652 | platform | re-base, delete: translators (7.6) | 5.7 | T-3643 (delete also T-3644) |
| T-3653, T-3654 | platform | switch, delete: notification egress (7.3) | 5.5 | T-3643, T-3499 |

## 9. Related

- `docs/analysis/gateway-to-broker-map.md`, `docs/analysis/operation-matrix.yaml`, `docs/analysis/mcp-surface.md`, `docs/analysis/odata-linkml-mapping.md`
- `docs/adr/ADR-0020-policy-seam.md`, `docs/adr/ADR-0023-endpoint-views.md`, `docs/adr/ADR-0027-surfaces-architecture.md`
- `/workspace/tasks/GOAL-antares-surfaces.md`, `/workspace/tasks/T-3525-analysis-platform-switch-to-antares-surfaces.md`
