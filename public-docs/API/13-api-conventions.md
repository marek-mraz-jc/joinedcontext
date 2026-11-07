---
sidebar_position: 14
title: API Conventions
description: The rules every configuration kind and Portal route follows: the same verbs, one list filter, cursor paging, ETag and If-Match, problem+json, validate-only, a version per kind and one shape for long-running work; a new feature is a kind or a view, never a new API style.
---

# API Conventions

[ADR-N-063](../Decisions/adr-n-063-one-api-style-no-graphql.md) gives the platform one API style per plane: NGSI-LD and its standard views for context data, resource REST for configuration, generic MCP tools over both. This page is the rule set of the resource REST plane. A data view follows its own standard instead ([API/02](02-endpoint-representations.md)); an MCP tool takes the same arguments as the REST operation it maps to and answers the same body.

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

A new feature is a new kind, a new operation of the registry (`joinedcontext-portal/src/ops/mod.rs`) or a new data view. It follows §1 to §7 and gets its REST route, its OpenAPI entry, its JSON Schema, its MCP tool definition and its UI form from the registry, never written by hand twice. A new API style needs a decision record that supersedes ADR-N-063.

## Related

- [ADR-N-063](../Decisions/adr-n-063-one-api-style-no-graphql.md): why one style per plane and no GraphQL.
- [API/01 §4](01-portal-api.md#4-resource-api-mf-11mf-15): the resource API as built.
- [API/09](09-errors.md): the problem types.
- [API/10](10-api-change-policy.md): the notice period for breaking changes.
- [API/02](02-endpoint-representations.md): the data views and their standards.
