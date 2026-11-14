---
sidebar_position: 17
title: "Portals API"
description: The operations that list, create, change and delete an Organization's open data portals, the addresses a portal answers on, the refusals and the isolation each operation keeps.
---

# Portals API

A `Portal` is an Organization-scoped manifest kind (ODP-08): one open data portal with its address, title, languages, appearance, DCAT-AP profiles and sources. The resource API of [API/01 §4](01-portal-api.md#4-resource-api-mf-11mf-15) serves it in namespace `org`, and every write is a Change of that Organization ([API/01 §5](01-portal-api.md#5-change-proposals-and-approvals-cc-34-cc-41-cc-63-ui-23ui-25)). The person's guide is [User Guide: Open Data Portals and Sources](../User-Guide/22-open-data-portals-and-sources.md).

## 1. Operations

| Operation | Route | Who |
|---|---|---|
| List the Organization's portals | `GET /api/v1/projects/org/portals` | members of the Organization; operators see every Organization's |
| Read one portal | `GET /api/v1/projects/org/portals/{name}` | the same |
| Propose a new or changed portal | `POST /api/v1/projects/org/changes` with the `Portal` manifest | Organization administrators; operators |
| Propose deleting a portal | `DELETE /api/v1/projects/org/portals/{name}` | Organization administrators; operators |
| List the hosts a portal may take | `GET /api/v1/organization/portal-hosts`: per Organization the caller may propose a Portal in, its `organization`, the `hosts` the operator listed and its `opendata` base address; `403` when there is none (ODP-15, T-6875) | Organization administrators; operators |

A request without a session answers `401`. A person who is not an administrator of the portal's Organization answers `403` on every write, and a read of another Organization's portal answers `404`, as if it did not exist (ODP-12). An Organization administrator's Change touches only that Organization's portals.

The reconciler is the only writer of the catalogue rows behind a portal (ODP-11). It applies an approved Change in-process: it creates, updates or deletes the portal's catalogue and its harvest sources without a restart. The `[[dataroom.harvest]]` seeds of the deployment configuration migrate once, with every field, into sources of each Organization's `default` portal; after that the config block is refused naming ODP-11. The direct `harvest_source.propose` operation is refused, naming ODP-11. No route of the open data surface gains a write verb for it, and each portal's catalogue has its own publish principal, so the principal of one portal cannot write another.

## 2. Addresses

| Portal | Address |
|---|---|
| `default` | `/{org}/opendata/` on the shared host, or the Organization's open data host |
| any other, no `spec.host` | `/{org}/opendata/-/{path}/` on the shared host, `/-/{path}/` on the Organization's open data host |
| any other, with `spec.host` | `https://{host}/`, a host the operator listed for the Organization; its path address answers `308` there (`spec.slug: redirect`, the default) or serves too (`keep`) |

A portal with `spec.host` sets `spec.slug`: `redirect` (the default) answers the path address `/{org}/opendata/[-/{path}/]...` with `308` to the same path and query on the host; `keep` serves both, and every page and feed names the host as canonical. Changing a portal's address never changes a dataset IRI: every dataset without an identifier of its own is pinned to the address it was first published under, and that address keeps answering (ODP-08).

When one Endpoint is listed in several portals of an Organization, the `default` portal's IRI is canonical and the other portals point their record at it; two sources harvesting one dataset are deduplicated by `dct:identifier` in source order: the earlier source keeps the record (ODP-14).

The catalogue row of a path-mode portal stores the synthetic host `{path}.{org}.portal.invalid`: the `.invalid` top-level domain never resolves, so no request can reach a portal through it. The deployment-wide catalogue at `/opendata/` is the `default` portal of the installation Organization (`ops`).

## 3. Validation

`jcctl validate` and the Portal check every `Portal` manifest the same way at propose, approve and load, and refuse with a Problem naming the field:

| Field | Rule | Refusal |
|---|---|---|
| `metadata.name` | a DNS label (1 to 63 lower-case letters, digits and hyphens, a letter or digit at each end), unique in namespace `org` | `422`, field `metadata.name` |
| `spec.organizationRef` | an Organization of the deployment, never changed on an existing portal | `422`, field `spec.organizationRef` |
| `spec.path` | a DNS label, unique in the Organization, never changed on an existing portal (a new path is a new catalogue and would move every dataset IRI); `default` is the Organization's own portal | `422`, field `spec.path` |
| `spec.host` | one of the hosts the deployment configuration lists for this Organization, checked against every mount host, and no other portal's | `422`, field `spec.host`, "host is not listed for this Organization" or "another portal answers on" |
| `spec.slug` | `redirect` or `keep`, only with `spec.host` | `422`, field `spec.slug` |
| total count | at most `portals.max` across every Organization of the deployment | `422`, "portals.max (N) reached" |
| delete of the portal at path `default` | never | `422`, "the default portal cannot be deleted" |
| `spec.sources[].url` | `https://` (the harvest guard fetches nothing else), its host inside `allowedHosts` and the deployment's harvest allow-list | `422`, field `spec.sources[i].url` |
| `spec.sources[].projects` | an `endpoints` source names only projects of the portal's Organization | `422`, field `spec.sources[i].projects` |
| unknown field | `deny_unknown_fields` | `422`, the field named |

Sources carry no credentials in this version: a source field that would hold a secret does not exist, and a `secretRef` in a source is refused as an unknown field.

## 4. Deleting a Portal

An approved delete of a portal other than `default` removes the portal's datasets, publishers and harvest sources from its catalogue. Every address of a removed dataset answers `410 Gone` with a Problem of type `gone` (ODP-09); the Endpoints that were listed stay in their projects untouched.

## 5. Isolation

Each JC Organization has its own dataroom organization id, so the catalogue store's row level security and the cross-organization harvest check keep two Organizations apart in the database (ODP-12). The proof runs as a `NOSUPERUSER NOBYPASSRLS` role: a query bound to Organization A's catalogue counts none of Organization B's rows, and a harvest run of A cannot write into B's catalogue.

## 6. Sources, runs and datasets

These routes serve the Sources and Datasets tabs of the Portals area (ODP-15). `{name}` is the Portal manifest's `metadata.name` and `{source}` a `spec.sources[].name` of it.

| Operation | Route | Right on the Portal manifest | Answer |
|---|---|---|---|
| Test a source's connection | `POST /api/v1/projects/org/portals/{name}/sources/{source}/test` | propose | `200` with `outcome`, one of `ok`, `unreachable`, `refused host`, `not the expected format`, `too large` |
| Run a source now | `POST /api/v1/projects/org/portals/{name}/sources/{source}/runs` | propose | `202` when the run started; `409` while a run of this source is active |
| List a source's last runs | `GET /api/v1/projects/org/portals/{name}/sources/{source}/runs` | read | `200` with at most 50 runs, newest first: `id`, `state`, `startedAt`, `finishedAt`, the created, updated, unchanged, withdrawn and skipped counts, and `error`, one word |
| List a portal's datasets | `GET /api/v1/projects/org/portals/{name}/datasets?offset=0&limit=50` | read | `200` with `total` and `items`: `name`, `title`, `source` (the harvest source, or `endpoints`) and `hidden` (named in `spec.hidden`) |

The test fetches the source's first page once through the harvest guard, the same guard a run uses: `https` only, the host inside the source's `allowedHosts` and the deployment's harvest allow-list, every address of the pinned DNS answer public, every redirect hop checked again. It reads at most the source's byte budget and stops after 10 seconds. It writes nothing. The answer is one word: no remote address, status line, header or body reaches it or the log.

A run's `error` is one word too: `unreachable`, `refused host`, `not the expected format`, `too large`, `over budget`, `past the time cap` or `failed`. The run log of the catalogue keeps the detail for the operator.

Refusals:

| Case | Answer |
|---|---|
| no session | `401` |
| a member of the portal's Organization without the right in the table | `403` |
| a portal or source of another Organization, or one that does not exist | `404`, the same body for both |
| more than 3 tests at once, or more than 6 a minute, by one person | `429` with `Retry-After` |
| this installation serves no open data catalogue | `503` |

The limits (6 tests a minute with a burst of 3, 50 runs, 10 seconds) are constants of the deployment.

## Related

- [API/01: Portal API](01-portal-api.md)
- [API/08: DCAT profiles](08-dcat-profiles.md)
- [API/15: Addresses v1](15-addresses-v1.md)
- [User Guide: Open Data Portals and Sources](../User-Guide/22-open-data-portals-and-sources.md)
