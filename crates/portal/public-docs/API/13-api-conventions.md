---
sidebar_position: 14
title: API Conventions
description: The rules of the management API: one OData 4.01 service per Organization at /{org}/api/v1/ (entity sets for reads, actions that open a Change for writes), and the REST compat routes it replaces with their list filter, cursor paging, ETag and If-Match, problem+json, validate-only, a version per kind and one shape for long-running work.
---

# API Conventions

[ADR-N-063](../Decisions/adr-n-063-one-api-style-no-graphql.md) gives the platform one API style per plane: NGSI-LD and its standard views for context data, generic MCP tools over both. [ADR-N-070](../Decisions/adr-n-070-odata-management-api-per-organization.md) makes the configuration plane one OData 4.01 service per Organization (§2a); the resource REST routes of §1 to §7 stay as compat wrappers over the same operations, frozen: nothing is added to them, and an [API/10](10-api-change-policy.md) notice ends them. This page is the rule set of both. A data view follows its own standard instead ([API/02](02-endpoint-representations.md)); an MCP tool takes the same arguments as the REST operation it maps to and answers the same body.

Each section says what the rule is and, where today's Portal differs, what it does now. T-5965 moves the routes onto the rules; a route that keeps an exception names its reason in its own API section.

## 1. Paths and verbs

A configuration kind is a collection under one prefix; `apiVersion` and `kind` live only inside the manifest body.

```text
GET    /api/v1/projects/{project}/{plural}             list
GET    /api/v1/projects/{project}/{plural}/{name}      one manifest with its status
POST   /api/v1/projects/{project}/{plural}             create   → 202 + Change
PUT    /api/v1/projects/{project}/{plural}/{name}      replace  → 202 + Change
PATCH  /api/v1/projects/{project}/{plural}/{name}      merge patch → 202 + Change
DELETE /api/v1/projects/{project}/{plural}/{name}      delete   → 202 + Change
```

- `{plural}` is the kind's lower-case plural (`endpoints`, `pipelines`, `policies`); `{name}` is `metadata.name`.
- An action other than create, read, update or delete is a `POST` to a sub-resource: `…/agent-runs/{id}/cancel` cancels a run, `…/changes/{id}/approve` records a decision. Custom-method suffixes (`…/{name}:run`) are not used.
- Reads are `GET` and never change state, so the edge caches them and rate-limits them by route class. A read never moves to `POST` for a long query; the query moves to a saved resource instead.

## 2. Listing: filter, search, sort, paging, count

```text
GET /api/v1/projects/{project}/pipelines?filter=spec.schedule:*%20AND%20labels.team="air"&q=ozone&sort=-metadata.name&limit=50&count=true&cursor=…
```

Every Portal list takes the same five list parameters, read by one extractor (`joinedcontext-portal/src/api/list_query.rs`); a route adds its own (`filter`, selectors) beside them.

| Parameter | Rule |
|---|---|
| `limit` | rows per page, 1 to 1000; any other value is `400` `invalid-request` with `errors[]` naming `limit`; without `limit` a resource list answers every readable object in one page |
| `cursor` | the opaque `nextCursor` of the previous answer; a cursor from another filter, sort, `q` or caller is `400` naming `cursor` |
| `sort` | one field of the rows, `-` in front for descending (`sort=-metadata.name`); default `metadata.name`. A malformed value is `400` naming `sort`; a field the rows do not have (the kind's jc-core JSON Schema decides) is `422` `unprocessable` naming `sort` |
| `q` | text, at most 200 characters, searched case-insensitively in each object's `metadata.name`, `metadata.title`, `metadata.labels` and `spec`, never in `status` |
| `count` | `true` adds `totalCount`, the rows of every page together after the read check, `filter` and `q`; `true` or `false`, anything else is `400` naming `count` |
| `filter` | a subset of [AIP-160](https://google.aip.dev/160): `field = value`, `!=`, `<`, `<=`, `>`, `>=`, `:` (has; `field:*` is present), `AND`, `OR`, `NOT`, parentheses, quoted strings. Fields are dotted manifest paths (`metadata.name`, `spec.space`) and `labels.{key}`. A field the kind's schema lacks or a malformed expression is `400` `invalid-request` naming the field and its position |

Each parameter is sent at most once; a second copy is `400` naming it. The answer is `{ "items": [...], "nextCursor": "…", "totalCount": 12 }`; `nextCursor` is absent on the last page and `totalCount` without `count=true`. A cursor is signed and carries the filter, sort and search it was made for, so it cannot be edited into another query.

`orderBy` (`field` or `field desc`) is the old name of `sort` and stays an alias until the notice of §7 ends; sending both is `400`.

A list counts and pages only over the objects the caller may read: the filter runs after `may_read_manifest`, so an unreadable object never shifts a page, shows in a total or turns a `400` into a `200`. A caller who may read nothing of the kind gets an empty list, and a caller who may not read the project `404`.

Every kind's list (`GET /api/v1/projects/{project}/{plural}`) is one handler over the kind catalogue (`joinedcontext-portal/src/api/resources.rs`); the organization's Endpoints list (`GET /api/v1/endpoints`) takes the same parameters, orders by project and then name by default and breaks a tie in a `sort` the same way. Both judge the list parameters after the read check, so a project the caller may not read is `404` whatever the query says. The filter checks its fields against the kind's jc-core JSON Schema (`joinedcontext-portal/src/resource/filter.rs`). A cursor is signed with a key of the running Portal, so a restart ends an open listing with a `400` that says to list again. The answer also keeps `metadata.continue` and `metadata.remainingItemCount`, and the list still takes `labelSelector`, `fieldSelector` and `continue` (Kubernetes syntax, `joinedcontext-portal/src/resource/selector.rs`) until the notice of §7 ends; `continue` pages by `metadata.name` only. Lists that are not a kind keep their own parameters until they move to the extractor: activity and agent-run lists use `cursor`, catalogue and change-history lists use `page`.

**OData per project (T-6632, T-6655).** The alias of §2a's project navigations until the API/10 notice ends. Every project is one OData 4.01 service generated from the operation registry, for OData clients (Excel, Power BI, Microsoft.OData.Client): the service document at `/api/v1/projects/{project}/odata/`, the CSDL model at `…/odata/$metadata`. Each list operation is an entity set: `jc_resource_list` once per kind under the kind's REST plural (`…/odata/endpoints`, `…/odata/pipelines`), `jc_change_list` as `changes` and `jc_workspace_list` as `workspaces`; the entity type holds the scalar members of the operation's output rows, key `name`. A set answers `$filter` (`eq`, `ne`, `gt`, `ge`, `lt`, `le`, `in`, `and`, `or`, `not`, `contains`, `startswith`, `endswith`), `$select`, `$orderby`, `$top`, `$skip`, `$count`, `/$count` and a key (`endpoints('air-counters')`); an answer holds at most 1000 rows and `@odata.nextLink` names the next page by `$skiptoken`. A name the model lacks is `400`, an option outside that list (`$expand`, `$search`, `$apply`, `$compute`) `501`, both as an OData error body (`{"error": {"code", "message"}}`). Each writing operation is an unbound action of its own name, its parameters the operation's input schema: `POST …/odata/jc_resource_propose` runs what `POST …/ops/jc_resource_propose` runs. A read calls the list operation through the registry, so its permission check and rows are the REST door's and MCP's: the check runs before any option, a `$filter` only narrows what the caller may read, and a project the caller may not read is `404`. The list operations not mapped yet and why are in T-6655.

## 2a. The OData service per Organization

[ADR-N-070](../Decisions/adr-n-070-odata-management-api-per-organization.md) decides this service; T-6809 builds it in slices. The Portal UI, the SDK and jcctl call it; MCP tools run the same operations.

```text
GET  /{org}/api/v1/                                              service document, per caller
GET  /{org}/api/v1/$metadata                                     CSDL XML (JSON with $format=application/json), per caller
GET  /{org}/api/v1/projects?$filter=…&$orderby=…&$top=50&$count=true
GET  /{org}/api/v1/projects/air/endpoints?$select=name,audience  key-as-segment; = projects('air')/endpoints
GET  /{org}/api/v1/endpoints(project='air',name='counters')      organization-wide set, composite key
GET  /{org}/api/v1/projects('air')?$expand=endpoints             one level
POST /{org}/api/v1/projects('air')/JoinedContext.Portal.propose_create                          → Change
POST /{org}/api/v1/projects('air')/endpoints('counters')/JoinedContext.Portal.propose_update     If-Match → Change
POST /{org}/api/v1/changes('42')/JoinedContext.Portal.approve                                   If-Match → Change
GET  /-/api/v1/organizations                                     installation scope, operators only
```

| Rule | What the service does |
|---|---|
| Organization | decided once from the `/{org}/` segment (shared host), the bound host's row (root mapped to `/{org}/`) or the single Organization (path off); never a header, query option, key or URN. Unknown id, or a path id on a host bound to another: `404` with the unknown-path body. Another Organization's token or session: `401`. Another Organization's key: `404` |
| Sessions | path mode shares one origin: the session cookie (`Path=/{org}/`) and the CSRF token are bound to the Organization and checked on every request |
| Reads | `projects` keyed by `name`; each project kind a navigation of its project and an organization-wide set keyed by `(project,name)`; organization kinds (`roles`, `rolebindings`, `groups`, `members`) sets of the service; the Organization and `me` singletons; set names are the REST plurals; a read with arguments is a function (§2b). Installation Roles, RoleBindings and the Organization list only at `/-/api/v1/`, to operators |
| Writes | a Change-proposing operation (§2b) is an action named by it: `propose_update`, `propose_delete` bound to an entity, `approve`, `reject`, `apply` bound to a Change, `propose_create` bound to its project or unbound for an organization kind; each answers the Change; `dryRun` answers the plan. Any other action keeps its registry permission and lane and opens no Change. `POST` to a set, `PATCH`, `PUT`, `DELETE` and deep insert are `405` naming the action |
| Concurrency, caching | `@odata.etag` = a hash of the caller-visible projection and the revision (a hidden property's change does not move it); a bound write action needs `If-Match`: absent `428`, stale `412` with the current ETag, no Change opened. `Cache-Control: private`, `Vary: Authorization, Cookie`; `304` only after the authorization check |
| Query options | `$filter` (`eq`, `ne`, `gt`, `ge`, `lt`, `le`, `in`, `and`, `or`, `not`, `contains`, `startswith`, `endswith`), `$select`, `$orderby`, `$top`, `$skip`, `$count`, `/$count`, a key, `$expand` one level on a declared navigation; nested options, property paths and `$count` pass the readable-property check |
| Refused | `501`: `$apply`, `$search`, `$compute`, `$levels`, `$deltatoken` and delta links, lambda `any`/`all`, `/$ref`, `/$value`, `$batch`, an undeclared function or action. `400`: `$expand` deeper or on a non-navigation, `$top` or `$skip` over the limits, an unknown or repeated option, a name the caller's `$metadata` lacks (hidden, unknown and other-Organization names answer a byte-identical body). `406`: a format other than JSON (XML too for `$metadata`). Nothing is silently ignored |
| Limits | deployment-wide, never per Organization: `odata.page_max` 1000 (`Prefer: odata.maxpagesize` lowers it), `odata.read_max` 10,000 readable rows per request (past it `400`, never a cut set), `odata.skip_max` 10,000, `odata.filter_max_nodes` 64, `odata.filter_max_depth` 8, `odata.expand_depth` 1 |
| Paging | `@odata.nextLink` with an opaque `$skiptoken`, MAC-signed over Organization, caller, set, query hash and expiry (replay, tamper or another caller: `400` or `404`); every page re-runs the authorization and holds readable rows only; the link's base comes from the host registry or path root, never `Host` or `X-Forwarded-Host` |
| Errors | the OData error object, `application/json`: `{"error": {"code": <the API/09 problem type>, "message", "target", "details", "title", "status", "trace_id"}}`; REST compat keeps `application/problem+json` with the same `type` and `trace_id` |
| Authorization | the registry's `Caller::grants`, `Effective::may` and `Effective::may_read_manifest`, before any option; `$count` counts readable rows only; `$metadata` and the service document list only what the caller may use (a viewer's hold no write action; no `secretRef`, `sourceUrl` or personal property); anonymous `401` unless the Organization has a public Endpoint, then only the public sets |
| Compat | under `/{org}/api/v1/`, a request with `OData-Version`, `OData-MaxVersion`, a `$` option, `$metadata`, the service document, a function or an action is OData; any other request is REST compat with today's body; `Vary: OData-Version, OData-MaxVersion` |

## 2b. Operation classes

Every operation of the registry (`crates/portal/src/ops`, 105 operations at jc `75e834b`, 2026-11-12) takes one of four OData forms. A new operation names its class when it is registered.

| Class | OData form | Operations |
|---|---|---|
| Entity set, singleton or navigation | `GET` of a set, a key or a navigation | `jc_resource_list`, `jc_resource_get` (a set per kind), `jc_project_get`, `jc_project_revisions`, `jc_endpoint_list_all` (organization-wide `endpoints`), `jc_change_list`, `jc_change_get`, `jc_run_list`, `jc_run_get`, `jc_draft_list`, `jc_draft_get`, `jc_workspace_list`, `jc_workspace_get`, `jc_activity_list`, `jc_member_list`, `jc_person_list`, `jc_person_get` (people-admin), `jc_access_request_list`, `jc_organization_read` (singleton), `jc_permissions_me` (`me`), `jc_repository_list`, `jc_repository_commits`, `jc_repository_commit`, `jc_repository_refs`, `jc_repository_pulls`, `jc_repository_pull` |
| Function (read-only, with arguments) | `GET` of a bound or unbound function | `jc_change_history`, `jc_catalog_search`, `jc_knowledge_search`, `jc_knowledge_status`, `jc_model_sdm_search`, `jc_kpi_compute`, `jc_federation_graph`, `jc_model_source_get`, `jc_schema_get`, `jc_repository_tree`, `jc_repository_file`, `jc_repository_settings`, `jc_ckan_status`, `jc_syncsource_status`, `jc_pipeline_metrics`, `jc_pipeline_logs`, `jc_pipeline_inspect`, `jc_pipeline_validate`, `jc_app_services`, `jc_space_governance`, `jc_serviceaccount_access`, `jc_access_explore`, `jc_access_simulate`, `jc_attribute_explain`, `jc_endpoint_preview`, `jc_catalogue_draft`, `jc_workspace_compare`, `jc_workspace_preview_get`, `jc_mqtt_captured`, `jc_project_export`, `jc_data_query` (through the Endpoint's view, ADR-N-050) |
| Change-proposing action | `POST` of an action; opens, plans or decides a Change; bound ones need `If-Match` | `jc_resource_propose`, `jc_resource_delete`, `jc_endpoint_propose`, `jc_model_source_put`, `jc_project_create`, `jc_project_delete`, `jc_project_import`, `jc_space_complete`, `jc_flow_start`, `jc_workspace_propose`, `jc_access_request_decide`, `jc_syncsource_detach`, `jc_change_approve`, `jc_change_reject`, `jc_change_apply` |
| Other action (no Change; keeps its registry permission and lane) | `POST` of an action | runs: `jc_run_create`, `jc_run_cancel`, `jc_run_publish`, `jc_run_answer`, `jc_run_message`, `jc_pipeline_run`, `jc_syncsource_sync`, `jc_syncsource_pause`; drafts and sessions: `jc_draft_put`, `jc_draft_drop`, `jc_workspace_open`, `jc_workspace_update_from_main`, `jc_workspace_discard`, `jc_workspace_preview_start`, `jc_workspace_preview_stop`; dry runs and computations: `jc_change_plan`, `jc_change_revert_plan`, `jc_manifest_dry_run`, `jc_datasource_check`, `jc_pipeline_test`, `jc_pipeline_sample_source`, `jc_pipeline_try_mapping`, `jc_mqtt_test_publish`, `jc_model_infer`, `jc_model_generate`, `jc_model_sdm_import`; people in the identity provider (people-admin): `jc_person_create`, `jc_person_edit`, `jc_person_disable`, `jc_person_enable`, `jc_person_sign_out`; data through an Endpoint's Policy: `jc_data_write`, `jc_thing_write` |

The drafts' event channel (`jc_draft_events`, an SSE stream) is no operation and stays on its transport, on the UI gate's allow-list beside OIDC, MCP, WebSocket and uploads.

## 3. Concurrency: ETag and If-Match

- `GET` of one manifest answers `ETag: "<SHA-256 of the manifest as JSON, without status>"`; `status` is computed by the Portal and never part of the stored manifest. `GET` with `If-None-Match` of the same value answers `304` with no body.
- `PUT`, `PATCH` and `DELETE` take `If-Match`. A stale value is `412` `precondition-failed` with the current `ETag` in the answer, and no Change is opened. A write without `If-Match` is accepted in `v1` (clients that never sent one keep working); `apply_change` always sends the plan's value.
- An `ETag` is strong and changes with every byte of the stored manifest.

An `If-Match` is compared only against a manifest the caller may read; for any other the write answers what it answers without one (`404` or `403`), so a `412` never tells that an unreadable object exists.

## 4. Errors

Every error is `application/problem+json` (RFC 9457) with `type`, `title`, `status`, `detail` and, for a refused field, `errors: [{ "pointer": "/spec/schedule", "detail": "…" }]` with a JSON Pointer into the request body. `type` is one of the problem types of [API/09](09-errors.md); a client branches on `type`, never on `detail`. A refusal says what the caller can do next (the field, the allowed values, the right that is missing) and never echoes a secret it was sent.

| Status | When |
|---|---|
| `400` | the request is malformed, a filter or cursor is bad, a manifest fails its schema |
| `401` | no valid token or session |
| `403` | the caller may see the object but not do this to it |
| `404` | the object does not exist or the caller may not read it (the two are not told apart) |
| `409` | the change conflicts with an open Change or the repository |
| `412` | `If-Match` is stale |
| `429` | over the route class's rate, with `Retry-After` |

## 5. Validate-only

Every write takes `?dryRun=All`. The Portal runs every check the write would run (schema, references, policy, the lane decision, the diff against the branch head) and answers `200` with the plan and the field errors; it opens no Change, writes nothing and calls no outside system except a read-only probe the kind's API section names. `plan_change` is this call; `apply_change` sends the same body with the plan's `ETag` as `If-Match`, so a plan that went stale is refused rather than applied.

A dry run is decided after authorization: a caller who may not write the kind gets `403`, not a plan.

## 6. Long-running work

A write that a person reviews or a job that runs longer than a request answers `202 Accepted` with `Location` of the resource that tracks it (a Change, a run, an erasure) and that resource in the body. The tracking resource has `status.phase` (`pending`, `running`, `succeeded`, `failed`, `cancelled`), `status.message` and `status.updatedAt`, and its `GET` is the poll. A client waits by polling with `If-None-Match` or by the resource's event stream where its API section names one. Cancelling is `POST …/{id}/cancel`.

## 7. Versions

- A kind is versioned by its `apiVersion` inside the manifest (`joinedcontext.com/v1`). A breaking change of one kind is `v2` of that kind, served beside `v1` with a conversion both ways; the URL prefix `/api/v1` stays for every kind.
- Adding an optional field, a kind, a route or a problem type is not breaking.
- Removing or renaming a field or parameter is announced with `Deprecation` and `Sunset` headers for 90 days first, as [API/10](10-api-change-policy.md) sets for public Endpoints.

## 8. What a new feature adds

A new feature is a new kind, a new operation of the registry (`joinedcontext-portal/src/ops/mod.rs`) or a new data view. It follows §2a and gets its entity set or action, its `$metadata` entry, its OpenAPI entry, its JSON Schema, its MCP tool definition and its UI form from the registry, never written by hand twice. No REST route is added (ADR-N-070). A new API style needs a decision record that supersedes ADR-N-070.

## Related

- [ADR-N-070](../Decisions/adr-n-070-odata-management-api-per-organization.md): OData as the management API, one service per Organization.
- [ADR-N-063](../Decisions/adr-n-063-one-api-style-no-graphql.md): why one style per plane and no GraphQL.
- [API/01 §4](01-portal-api.md#4-resource-api-mf-11mf-15): the resource API as built.
- [API/09](09-errors.md): the problem types.
- [API/10](10-api-change-policy.md): the notice period for breaking changes.
- [API/02](02-endpoint-representations.md): the data views and their standards.
