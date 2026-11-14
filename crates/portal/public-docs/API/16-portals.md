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

A request without a session answers `401`. A person who is not an administrator of the portal's Organization answers `403` on every write, and a read of another Organization's portal answers `404`, as if it did not exist (ODP-12). An Organization administrator's Change touches only that Organization's portals.

The reconciler is the only writer of the catalogue rows behind a portal (ODP-11). It applies an approved Change in-process: it creates, updates or deletes the portal's catalogue and its harvest sources without a restart. No route of the open data surface gains a write verb for it, and each portal's catalogue has its own publish principal, so the principal of one portal cannot write another.

## 2. Addresses

| Portal | Address |
|---|---|
| `default` | `/{org}/opendata/` on the shared host, or the Organization's open data host |
| any other, no `spec.host` | `/{org}/opendata/-/{name}/` on the shared host |
| any other, with `spec.host` | `https://{host}/`, a host the operator listed for the Organization |

The catalogue row of a path-mode portal stores the synthetic host `{name}.{org}.portal.invalid`: the `.invalid` top-level domain never resolves, so no request can reach a portal through it. The deployment-wide catalogue at `/opendata/` is the `default` portal of the installation Organization (`ops`).

## 3. Validation

`jcctl validate` and the Portal check every `Portal` manifest the same way at propose, approve and load, and refuse with a Problem naming the field:

| Field | Rule | Refusal |
|---|---|---|
| `metadata.name` | a DNS label (1 to 63 lower-case letters, digits and hyphens, a letter or digit at each end), unique in the Organization | `422`, field `metadata.name` |
| `spec.host` | one of the hosts the deployment configuration lists for this Organization, checked against every mount host | `422`, field `spec.host`, "host is not listed for this Organization" |
| total count | at most `portals.max` across every Organization of the deployment | `422`, "portals.max (N) reached" |
| delete of `default` | never | `422`, "the default portal cannot be deleted" |
| `spec.sources[].url` | `https://` or `http://`, its host inside `allowedHosts` and the deployment's harvest allow-list | `422`, field `spec.sources[i].url` |
| unknown field | `deny_unknown_fields` | `422`, the field named |

Sources carry no credentials in this version: a source field that would hold a secret does not exist, and a `secretRef` in a source is refused as an unknown field.

## 4. Deleting a Portal

An approved delete of a portal other than `default` removes the portal's datasets, publishers and harvest sources from its catalogue. Every address of a removed dataset answers `410 Gone` with a Problem of type `gone` (ODP-09); the Endpoints that were listed stay in their projects untouched.

## 5. Isolation

Each JC Organization has its own dataroom organization id, so the catalogue store's row level security and the cross-organization harvest check keep two Organizations apart in the database (ODP-12). The proof runs as a `NOSUPERUSER NOBYPASSRLS` role: a query bound to Organization A's catalogue counts none of Organization B's rows, and a harvest run of A cannot write into B's catalogue.

## Related

- [API/01: Portal API](01-portal-api.md)
- [API/08: DCAT profiles](08-dcat-profiles.md)
- [API/15: Addresses v1](15-addresses-v1.md)
- [User Guide: Open Data Portals and Sources](../User-Guide/22-open-data-portals-and-sources.md)
