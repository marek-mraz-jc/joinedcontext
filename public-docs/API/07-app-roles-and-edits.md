---
sidebar_position: 8
title: "Roles and Edits in a Server App"
description: How a server WASM App decides who may change what from the caller's roles, keeps rows per owner, records every write, and edits context data through its own Endpoint.
---

# Roles and Edits in a Server App

A server WASM App (`spec.kind: wasm`) learns the person a request comes from through `identity::caller()` and nothing else: an id, a name, an e-mail and the App's roles this person holds, as the Portal verified them ([AP-109](../Requirements/apps.md), [AP-147](../Requirements/apps.md)). The Rust SDK `jc_app_sdk` turns that into four tools: role checks, rows that belong to their author, a history of every write, and edits of context data that the App's Endpoint Policy decides a second time. This page is the contract for App authors and for the builder's pack (T-3825).

## 1. Who is asking

| Call | Answer |
|---|---|
| `Caller::require()` | the signed-in caller, or a `401` problem: "sign in to do this" |
| `caller.has_role("steward")` | whether the caller holds the App role `steward` |
| `caller.require_role(&["steward", "editor"])` | `Ok` when the caller holds one of them, else a `403` problem naming the roles this needs |

Rules:

1. Roles come from `identity::caller()` alone. A role in a request body, a query parameter or a header is data, never a right; the SDK reads none.
2. A `403` names the roles the action needs and never the caller's own roles, another person or a row.
3. A job (AP-154) and an anonymous visitor have no caller: every helper below answers `401`, so a write route needs no extra branch for them.

## 2. Rows that belong to their author

A table the App keeps per person has `id bigserial primary key` and `owner_id text not null`. `Owned::new("notes", "editor")` names such a table and the App role that may change every row of it. Table and column names are fixed by the App's code: the SDK accepts `[a-z_][a-z0-9_]*` up to 63 characters and refuses `id` and `owner_id` as columns the caller sets. Every value travels as a bound parameter.

| Call | Statement | Answer |
|---|---|---|
| `owned.insert(&caller, &["body"], values, summary)` | inserts with `owner_id` set to `caller.id` | the new row |
| `owned.update(&caller, id, &["body"], values, summary)` | changes row `id` when the caller owns it or holds the editor role | `Ok`, or `404` when no row changed |
| `owned.delete(&caller, id, summary)` | deletes row `id` under the same rule | `Ok`, or `404` when no row went |

A row the caller may not change answers the same `404` as a row that does not exist, so a request learns nothing about another person's rows.

## 3. History

Every helper of section 2 writes one row into the App's table `app_audit` in the same statement as the change: a data-modifying `WITH` that the host runs as one statement (AP-144), so the change and its history row commit together or not at all. The App creates the table in a migration of its own (AP-149) with the SDK's `AUDIT_TABLE` text:

```sql
create table app_audit (
    id bigserial primary key,
    at timestamptz not null default now(),
    actor_id text not null,
    actor_name text not null,
    action text not null check (action in ('insert', 'update', 'delete', 'edit')),
    target text not null,
    summary text not null check (length(summary) <= 500)
);
```

`target` is `<table>/<id>` for a row and the Entity id for a context-data edit. `summary` is the App's own sentence, at most 500 characters; the SDK cuts a longer one. `history(limit)` reads the newest rows first, at most 200, for the App's History panel. `record(&caller, "edit", target, summary)` writes one row for a change outside the App's database (section 4).

## 4. Editing context data

`gateway::patch_entity(id, &attrs)` sends `PATCH /ngsi-ld/v1/entities/{id}/attrs` to the App's own Endpoint, with the caller's token added by the host (AP-147). The Endpoint's Policy decides whether this person may write these Attributes. A refusal comes back as the gateway's `401` or `403` and passes to the browser as such. The App's own role check stays in front of it; the Policy is the second check, which still holds when an App forgets the first.

## 5. Threats this answers

| Row | Threat | Answer | Test |
|---|---|---|---|
| R1 | a client sends `"role": "steward"` or a role header and edits | roles come from `identity::caller()` only (section 1) | a request carrying a role in its body and headers is refused |
| R2 | a viewer edits another person's row by id | `where owner_id = caller` unless the editor role, `404` either way (section 2) | non-owner `404`, owner and editor `204` |
| R3 | SQL built from a request | names fixed by code and checked, values bound (section 2) | an invalid name is refused before any statement |
| R4 | a write without a trace, or a trace without the write | change and history row in one statement (section 3) | a refused change leaves no history row |
| R5 | an App forgets a role check on a context-data edit | the Endpoint Policy decides the `PATCH` (section 4) | a viewer's `PATCH` is refused by the gateway |
| R6 | an error body lists roles or other people | `401` and `403` carry fixed sentences (section 1) | the bodies hold no caller role and no other person |

## Related

- [Platform Services for Apps](06-app-services.md), the other services of an App's host
- [ADR-N-044](../Decisions/adr-n-044-server-wasm-apps-on-a-shared-host.md), the shared host
- [Apps requirements](../Requirements/apps.md), AP-109, AP-144, AP-147, AP-149, AP-154
