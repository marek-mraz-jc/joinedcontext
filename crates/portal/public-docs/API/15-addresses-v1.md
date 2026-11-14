---
sidebar_position: 16
title: "Addresses v1"
description: The one URL contract of the joinedcontext platform; every public address under https://{host}/{org}/, every Endpoint surface path, the MCP tool names and the published prod addresses that stay served in place.
---

# Addresses v1

This page is the address contract a client can store for years: the grammar of every public URL, the path of every Endpoint surface, the names of the MCP tools, the issuer string, the stability class of each address, and the addresses prod published before version 1 that stay served (EP-180…EP-186). It is written before the routing code moves (docs first), so the routing task builds to the "v1" column and the route golden test holds the code to it. Readers: client and connector authors, operators who publish links, and whoever changes a route.

## 1. How to read this page

- **Class** is how stable a row is. `frozen`: people outside the platform hold it (Endpoint surfaces and feeds, `/{org}/data/` pages and dataset links, MCP addresses and tool names, the issuer, `/.well-known/` documents); a change needs an alias served in place. `versioned`: the management API, whose path shape is fixed and whose payload moves to OData (§4). `internal`: pages not built yet or used by operators only (`/-/` operator pages, `/{org}/git/`, `/{org}/dash/`, `/{org}/dsc/` until each is built).
- **Today** is what jc-portal-1 serves at commit `05911dba` (2026-11-11). **v1** is the address this contract fixes. Where the two differ, the routing task moves the code, and the today address becomes a kept address (§6).
- Code citations are relative to the jc repository at that commit, with these prefixes: `GW` = `vendor/joinedcontext-platform/crates/context-gateway/src/`, `PO` = `crates/portal/src/`, `DR` = `crates/dataroom/`, `JC` = `bins/jc/src/`, `KA` = `crates/kaisatsu/`. So `GW handlers/ogc.rs:1001` is the line that mounts the OGC surface.
- `{org}` is an Organization id (§2), `{project}` a Project name, `{slug}` an Endpoint slug, `{base}` the deployment's base domain, `{apps-base}` its Apps domain.

## 2. The grammar

Every address of an Organization has the Organization id as its first path segment, on every host (ARCHITECTURE-TARGET §7):

```text
https://{host}/{org}/                                   Portal UI
https://{host}/{org}/api/v1/                            management API (OData service root, REST compat paths)
https://{host}/{org}/api/mcp                            MCP management door
https://{host}/{org}/api/mcp/data                       MCP data hub
https://{host}/{org}/api/endpoint/{slug}/{surface}/...  one Endpoint on one surface (§3)
https://{host}/{org}/id/                                identity: issuer, sign-in, account, credentials
https://{host}/{org}/data/                              open data catalogue (dataroom)
https://{host}/{org}/git/   /{org}/dash/   /{org}/dsc/  forge, dashboards, data space connector
https://{host}/-/...                                    deployment pages: sign-in chooser, assets, docs, operator
https://{host}/.well-known/...                          discovery documents, ACME
https://{app}--{org}.{apps-base}/                       one App (one-domain deployment: /{org}/app/{app}/)
mqtts://mqtt.{base}:8883, wss://mqtt.{base}/            MQTT; Organization from the credential, never the topic
```

Rules every row follows:

- **The prefix rule.** The v1 address of every path the platform serves today without an Organization is `/{org}` followed by today's path, unless §4 names another. A gateway path keeps its whole suffix: `/api/endpoint/{slug}/ngsi-ld/v1/entities` becomes `/{org}/api/endpoint/{slug}/ngsi-ld/v1/entities`.
- **Unknown input.** A host not in the host registry answers `421`. An unknown Organization id, and a known one on a host bound to another Organization, answer `404` with the same body as an unknown path.
- **One decision.** The Organization of a request is decided once, from the first path segment against the registry, from the host row of a bound host, or from the MQTT credential, and nothing later reads the path, the host or a URN again to find it. A token whose `org` differs from the request's Organization is refused `401`.

### The Organization id

The id in the path is the Organization's verified domain with its labels reversed and joined by `_` (EP-186):

| Domain | Organization id |
|---|---|
| `bbsk.sk` | `sk_bbsk` |
| `bratislava.sk` | `sk_bratislava` |
| `praha.eu` | `eu_praha` |
| `hel.fi` | `fi_hel` |
| `banskabystrica.sk` | `sk_banskabystrica` |

The domains are confirmed when each Organization is created. An id matches `[a-z][a-z0-9_-]{1,62}` and contains `_`. A hostname holds no `_`, so the map from domain to id is one-to-one, and no route segment (`api`, `data`, `id`, `-`) can equal an id, so no reserved-word list exists. Only an operator mints an id, and only from a domain whose ownership a DNS TXT record verified: without the check anyone could take `com_google`. The id is a label: rows, tokens and logs carry the internal OrgId, and a later domain change keeps the id or adds an alias. An id in a `frozen` row is permanent; a rename keeps the old id answering in place for API, feed and MCP rows (`308` for pages), bound to the same OrgId.

Entity URNs carry the owning Organization's id, `urn:ngsi-ld:{Type}:sk_bbsk:{rest}` ([ADR-N-068](../Decisions/adr-n-068-urn-owner-prefix.md)). Ids minted before carry the domain, `urn:ngsi-ld:{Type}:bbsk.sk:{space}:{id}`, which names the same Organization and stays valid; no URN is minted again (an entity's identity is its Context Space and URN, and temporal history and outside links hold today's ids). Nothing derives an Organization or a right from a URN (ADR-N-041).

### Hosts and the two deployment settings

A host is *shared* (the path selects the Organization) or *bound* to one Organization. The operator chooses two settings per deployment and records them in its configuration:

- **Organization in the path.** `on` for a deployment with several Organizations: every address is `https://{host}/{org}/...`. `off` for a deployment with one Organization: the same paths without the prefix (`https://opendata.example.com/data/`, `https://api.example.com/api/endpoint/{slug}/...`). Turning it on later keeps every prefix-less address answering (API in place, pages `308`), so a single deployment grows without a broken link.
- **Host roots.** A bound host may map its root to a path prefix of its Organization, so an own domain repeats neither the Organization nor the role:

| Host row | Root maps to | Example address | Same as |
|---|---|---|---|
| `data.bratislava.sk` bound to `sk_bratislava` | `/sk_bratislava/` | `https://data.bratislava.sk/data/dataset/{name}` | `https://{shared-host}/sk_bratislava/data/dataset/{name}` |
| `opendata.example.com` bound to `com_example` | `/com_example/data/` | `https://opendata.example.com/dataset/{name}` | `https://{shared-host}/com_example/data/dataset/{name}` |
| `api.example.com` bound to `com_example` | `/com_example/api/` | `https://api.example.com/endpoint/{slug}/ngsi-ld/v1/entities` | `https://{shared-host}/com_example/api/endpoint/{slug}/ngsi-ld/v1/entities` |
| prod's `api.`, `opendata.`, `portal.` hosts (§6) | `/{org}/api/`, `/{org}/data/`, `/{org}/` | today's paths unchanged | the Organization comes from the row the path names (§6) |

Canonical addresses and permanence:

- On a bound host the prefix-less path is canonical. The prefixed form keeps answering on that host (API in place, pages `308` to the prefix-less form), so a move from the shared host breaks no link.
- The shared-host address of an Organization is permanent: an API call is served in place on every registered host, and a later own domain adds an address and removes none. A browser page answers `308` to the canonical host.

## 3. Endpoint surfaces

The v1 address is `/{org}/api/endpoint/{slug}/` followed by the path in the first column; today it is `/api/endpoint/{slug}/` followed by the same path, on the gateway's host. Every row is a route of the gateway today.

| Path below the Endpoint | Serves | Today's route |
|---|---|---|
| (empty) and `/` | the Endpoint record (DCAT-shaped JSON-LD) | `GW handlers/catalog.rs:372`, `:373` |
| `widget.json` | widget defaults of the view | `GW handlers/catalog.rs:374` |
| `datasets/{name}` | one dataset of `spec.catalog.datasets` | `GW handlers/catalog.rs:376` |
| `ngsi-ld/v1`, `ngsi-ld/v1/`, `ngsi-ld/v1/{*rest}` | NGSI-LD API (entities, temporal, types, attributes, subscriptions, batch) | `GW handlers/ngsi_ld.rs:1427`…`:1429` |
| `ogc/features`, `ogc/features/`, `ogc/features/{*rest}` | OGC API Features | `GW handlers/ogc.rs:1001`…`:1004` |
| `sta/v1.1`, `sta/v1.1/`, `sta/v1.1/{*rest}` | OGC SensorThings API 1.1 | `GW handlers/ogc.rs:1008`…`:1010` |
| `odata/v4`, `odata/v4/`, `odata/v4/{*rest}` | OData v4 service of the view | `GW handlers/ogc.rs:1012`…`:1014` |
| `mcp` | MCP server of one Endpoint | `GW mcp/mod.rs:34` |
| `.well-known/oauth-protected-resource` | RFC 9728 metadata of that MCP server | `GW mcp/mod.rs:36` |
| `ws` | WebSocket notifications | `GW handlers/ws.rs:477` |
| `file.csv` | the view as one CSV table (permanent alias of `entities.csv`, EP-177) | `GW handlers/files.rs:132` |
| `entities.csv` | one Query Entities as CSV | `GW handlers/ogc.rs:1029` |
| `entities.ics`, `entities.rss`, `entities.atom`, `entities.json` | one Query Entities as iCalendar, RSS 2.0, Atom, JSON Feed 1.1 (EP-174) | `GW handlers/ogc.rs:1023`…`:1026` |
| `feeds/{preset}.{ext}`, `ext` = `ics`, `rss`, `atom`, `json`, `csv` | a stored preset (EP-177) | `GW handlers/ogc.rs:1021`, `GW handlers/ogc.rs:178` |
| `feeds/{filter}/{preset}.{ext}` | the preset narrowed by a Query Entities query string in one path segment, at most 2,048 characters | `GW handlers/ogc.rs:246` (`preset_link`), `GW handlers/feeds_csv.rs:45` |
| `ical/v1/{preset}.ics` | permanent alias of `feeds/{preset}.ics` (EP-177) | `GW handlers/ogc.rs:1016` |
| `feed/v1/{preset}/atom.xml`, `rss.xml`, `feed.json` | permanent alias of `feeds/{preset}.{atom,rss,json}` | `GW handlers/ogc.rs:1018` |
| `schema/index.json`, `schema/{version}/{artifact}` | the model of the view per version (LinkML, JSON Schema, SHACL, context) | `GW handlers/schema_routes.rs:313`, `:315` |
| `schema/{version}/mappings/`, `schema/{version}/mappings/{file}` | the mappings of that model version | `GW handlers/mappings.rs:426`, `:430` |
| `openapi.json` | OpenAPI document of the Endpoint | `GW handlers/openapi_doc.rs:223` |
| `access`, `access/check`, `access/attributes`, `access/simulate`, `access/v1/evaluation`, `access/v1/evaluations` | what the caller may do here (AuthZEN-shaped) | `GW handlers/view_access.rs:961`…`:976` |

A Context Space has the same surfaces under `/{org}/cs/{space}/` (today `/cs/{space}/`: `GW handlers/space_routes.rs:85`, `GW handlers/ngsi_ld.rs:1430`, `GW handlers/schema_routes.rs:308`, `GW mcp/mod.rs:35`). `facets` (EP-178) is specified and not built yet; when it lands it takes `/{org}/api/endpoint/{slug}/facets`. A surface added later is one more path below the Endpoint and changes no row above. Every row of this section is `frozen`.

## 4. Management, identity, catalogue and module addresses

| Today | v1 | Kind | Class | Today's route |
|---|---|---|---|---|
| `portal.{base}/` and `/projects/{project}/...` | `/{org}/` and `/{org}/projects/{project}/...` | page | frozen for `/{org}/` and `/{org}/projects/{project}/`; deeper UI routes follow the UI | Portal UI router |
| `portal.{base}/api/v1/...` (303 operations, `/api/v1/projects/{project}/{plural}` among them) | `/{org}/api/v1/...` | API | versioned | `PO api/` (utoipa paths) |
| `portal.{base}/api/v1/mcp` | `/{org}/api/mcp` | MCP | frozen | `PO mcp/mod.rs:84` |
| `portal.{base}/c/{id}` (a person's connector) | `/{org}/c/{id}` | MCP | frozen | `PO mcp/connector.rs:39` |
| `{api host}/api/mcp` (data hub) | `/{org}/api/mcp/data` | MCP | frozen | `GW mcp/mod.rs:41`, `GW auth/subject.rs:386` |
| `{api host}/api/endpoints` (hub's Endpoint list) | `/{org}/api/endpoints` | API | frozen | `GW mcp/mod.rs:42` |
| `{api host}/api/mcp/{project}/{name}` (named MCP server) | `/{org}/api/mcp/{project}/{name}` | MCP | frozen | `GW mcp/mod.rs:62` |
| `{api host}/catalog.jsonld`, `/catalog.ttl`, `/catalog/{file}` | `/{org}/catalog.jsonld`, `/{org}/catalog.ttl`, `/{org}/catalog/{file}` | API | frozen | `GW handlers/catalog.rs:378`…`:381` |
| `{api host}/oauth/register`, `/oauth/authorize`, `/oauth/token` | `/{org}/oauth/...` | API | frozen | `GW oauth.rs:453`…`:456` |
| `id.{base}/realms/{realm}` and its OIDC paths | `/{org}/id/` | identity | frozen | `KA oidc/src/handlers.rs:47` |
| `opendata.{base}/` pages: `/dataset/{name}`, `/dataset/{name}/resource/{id}`, `/organization/{name}`, `/{lang}/...` | `/{org}/data/` + the same path | page | frozen | `DR api/src/lib.rs:1332`, `DR web/src/lib.rs:1455`, `:1665`, `:1958` |
| `opendata.{base}/api/3/action/{action}`, `/api/action/{action}` (CKAN API) | `/{org}/data/api/3/action/{action}` | API | frozen | `DR api/src/lib.rs:494` |
| `opendata.{base}/catalog`, `/catalog.{ttl,n3,jsonld,rdf,xml,nt}`, `/profiles/{profile}/catalog.*`, `/publisher/{publisher}/catalog.*` | `/{org}/data/` + the same path | API | frozen | `DR api/src/lib.rs:502`…`:508` |
| `opendata.{base}/feeds/dataset.atom`, `/feeds/organization/{file}`, `/feeds/tag/{file}` | `/{org}/data/feeds/...` | feed | frozen | `DR api/src/lib.rs:526`…`:532` |
| `opendata.{base}/odata/v4/{*rest}` | `/{org}/data/odata/v4/...` | API | frozen | `DR api/src/lib.rs:536` |
| `opendata.{base}/mcp` | `/{org}/data/api/mcp` (anonymous, read-only, public Endpoints) | MCP | frozen | `DR api/src/lib.rs:572` |
| `git.{base}/` or `{base}/git/` | `/{org}/git/` | Git, page | internal | forge (shako) mount, `JC module.rs:61` |
| `dashboards.{base}/` | `/{org}/dash/` | page | internal | `JC module.rs:61` |
| `assistant.{base}/d/{project}`, `/api/v1/d` | `/{org}/d/{project}`, `/{org}/api/v1/d` | page, API | frozen | `JC module.rs:97` |
| `{app}.apps.{base}` | `{app}--{org}.{apps-base}` (one domain: `/{org}/app/{app}/`) | page | frozen | Apps host |

The management API is `versioned`: only the Portal UI, `jcctl` and the MCP management door call it. Its path shape is fixed, because key-as-segment OData addressing (`/{org}/api/v1/projects/{project}/endpoints/{name}`) equals today's REST shape. Its payload moves to OData 4.01 ([ADR-N-070](../Decisions/adr-n-070-odata-management-api-per-organization.md), [API/13 §2a](13-api-conventions.md#2a-the-odata-service-per-organization)): a collection answers `{"value": [...]}` with `@odata.nextLink` instead of today's bodies, queries use `$filter`, `$select`, `$expand` (one level), `$orderby`, `$top`, `$skip` and `$count`, every entity carries `@odata.etag`, a write is an action that opens a Change and needs `If-Match` when it is bound to an entity, and an error is the OData error object carrying the Problem Details members. A request without `OData-Version`, `OData-MaxVersion` or a `$` option gets today's REST body until an API/10 notice ends it. Nothing outside the platform should build on today's REST bodies; a client outside uses the MCP tools (§5), whose names are `frozen`.

How a module gets its host today: `JC app.rs:164` (`plan_mounts`: slug mode puts a module under `/{module slug}` or its root paths, host mode on `{label}.{base}`) and `JC app.rs:244` (`organization_specs`: each `[[organizations]]` entry adds its own role hosts). Version 1 replaces both with the host registry and the prefix; the role hosts become kept addresses (§6).

### Identity and discovery

The issuer of an Organization is `https://{canonical-host}/{org}/id`, fixed by this page (EP-183). `{canonical-host}` is the host the Organization has when it is created (with "Organization in the path" `off`, the issuer is `https://{canonical-host}/id`), and the issuer stays that string when the Organization later binds an own domain or the setting changes: an issuer is an identifier, and changing it signs every person out and breaks every client that stored it. It is built from the registry, never from `Host`, `X-Forwarded-Host` or `Forwarded`. Discovery follows the path-issuer rules:

| Document | Address |
|---|---|
| OpenID configuration | `https://{canonical-host}/{org}/id/.well-known/openid-configuration` |
| RFC 8414 authorization server metadata | `https://{canonical-host}/.well-known/oauth-authorization-server/{org}/id` |
| RFC 9728 protected resource, management door | `https://{host}/.well-known/oauth-protected-resource/{org}/api/mcp` |
| RFC 9728 protected resource, data hub | `https://{host}/.well-known/oauth-protected-resource/{org}/api/mcp/data` |
| RFC 9728 protected resource, one Endpoint | `https://{host}/.well-known/oauth-protected-resource/{org}/api/endpoint/{slug}/mcp` |
| OpenID4VCI credential issuer metadata | `https://{canonical-host}/.well-known/openid-credential-issuer/{org}/id` |

Each document names only its own Organization. The suffix form a client may build (`{resource}/.well-known/oauth-protected-resource`, `GW mcp/mod.rs:36`, `:44`) stays served beside it.

## 5. MCP tool names

A tool name is `{kind}_{verb}` in lower snake case and does not change. Today's Portal tools carry a `jc_` prefix. In version 1, `tools/list` names each tool by its v1 name, and a `tools/call` naming today's `jc_*` name runs the same operation with the same schemas: the `jc_*` names are aliases with no end date. A rename after version 1 keeps the old name as an alias the same way.

The data door's tools (`query_entities`, `get_entity`, `query_temporal`, `retrieve_temporal`, `describe_schema`, `list_types`, `get_type`, `list_attributes`, `get_attribute`, `area_overview`, `search` and the writes a Policy grants) already have their v1 names and keep them on `/{org}/api/mcp/data`, `/{org}/api/endpoint/{slug}/mcp` and the named servers.

Rule for the table: the v1 name is today's name without `jc_`. Six tools get another name, because `resource`, `data` and `list_all` are no kind or verb: the generic resource tools become the set registry's `set_query`, `set_get` and `set_delete`, a resource proposal becomes `change_propose`, the Endpoint list becomes `endpoint_list`, and the Portal's data query becomes the data door's `query_entities` with its `endpoint` argument.

| Today | v1 | Registered at |
|---|---|---|
| `jc_access_explore` | `access_explore` | `PO ops/access_tools.rs:27` |
| `jc_access_simulate` | `access_simulate` | `PO ops/access_tools.rs:26` |
| `jc_activity_list` | `activity_list` | `PO ops/views.rs:380` |
| `jc_app_services` | `app_services` | `PO ops/runs.rs:275` |
| `jc_attribute_explain` | `attribute_explain` | `PO ops/access_tools.rs:28` |
| `jc_catalog_search` | `catalog_search` | `PO ops/compute.rs:427` |
| `jc_catalogue_draft` | `catalogue_draft` | `PO ops/access_tools.rs:30` |
| `jc_change_apply` | `change_apply` | `PO connectors.rs:194` |
| `jc_change_approve` | `change_approve` | `PO ops/changes.rs:318` |
| `jc_change_get` | `change_get` | `PO ops/views.rs:506` |
| `jc_change_history` | `change_history` | `PO ops/changes.rs:229` |
| `jc_change_list` | `change_list` | `PO ops/changes.rs:291` |
| `jc_change_plan` | `change_plan` | `PO connectors.rs:193` |
| `jc_change_reject` | `change_reject` | `PO ops/resources.rs:712` |
| `jc_change_revert_plan` | `change_revert_plan` | `PO connectors.rs:196` |
| `jc_ckan_status` | `ckan_status` | `PO ops/views.rs:602` |
| `jc_data_query` | `query_entities` | `PO ops/data_query.rs:19` |
| `jc_data_write` | `data_write` | `PO ops/data_write.rs:21` |
| `jc_datasource_check` | `datasource_check` | `PO ops/proposals.rs:947` |
| `jc_draft_drop` | `draft_drop` | `PO ops/drafts.rs:1152` |
| `jc_draft_get` | `draft_get` | `PO ops/drafts.rs:1081` |
| `jc_draft_list` | `draft_list` | `PO ops/drafts.rs:1116` |
| `jc_draft_put` | `draft_put` | `PO ops/drafts.rs:1016` |
| `jc_endpoint_list_all` | `endpoint_list` | `PO ops/views.rs:659` |
| `jc_endpoint_preview` | `endpoint_preview` | `PO ops/access_tools.rs:29` |
| `jc_endpoint_propose` | `endpoint_propose` | `PO ops/proposals.rs:801` |
| `jc_federation_graph` | `federation_graph` | `PO ops/views.rs:427` |
| `jc_flow_start` | `flow_start` | `PO ops/runs.rs:473` |
| `jc_knowledge_search` | `knowledge_search` | `PO ops/knowledge.rs:180` |
| `jc_knowledge_status` | `knowledge_status` | `PO ops/knowledge.rs:164` |
| `jc_kpi_compute` | `kpi_compute` | `PO ops/compute.rs:448` |
| `jc_manifest_dry_run` | `manifest_dry_run` | `PO ops/proposals.rs:866` |
| `jc_member_list` | `member_list` | `PO ops/people.rs:33` |
| `jc_model_generate` | `model_generate` | `PO ops/model_catalog.rs:26` |
| `jc_model_infer` | `model_infer` | `PO ops/compute.rs:499` |
| `jc_model_sdm_import` | `model_sdm_import` | `PO ops/model_catalog.rs:25` |
| `jc_model_sdm_search` | `model_sdm_search` | `PO ops/model_catalog.rs:24` |
| `jc_model_source_get` | `model_source_get` | `PO ops/views.rs:453` |
| `jc_model_source_put` | `model_source_put` | `PO ops/admin.rs:390` |
| `jc_mqtt_captured` | `mqtt_captured` | `PO ops/mqtt.rs:22` |
| `jc_mqtt_test_publish` | `mqtt_test_publish` | `PO ops/mqtt.rs:183` |
| `jc_permissions_me` | `permissions_me` | `PO ops/access_tools.rs:25` |
| `jc_person_create` | `person_create` | `PO ops/people.rs:421` |
| `jc_person_disable` | `person_disable` | `PO ops/people.rs:480` |
| `jc_person_edit` | `person_edit` | `PO ops/people.rs:447` |
| `jc_person_enable` | `person_enable` | `PO ops/people.rs:501` |
| `jc_person_get` | `person_get` | `PO ops/people.rs:401` |
| `jc_person_list` | `person_list` | `PO ops/people.rs:369` |
| `jc_person_sign_out` | `person_sign_out` | `PO ops/people.rs:522` |
| `jc_pipeline_inspect` | `pipeline_inspect` | `PO ops/pipeline_runs.rs:171` |
| `jc_pipeline_logs` | `pipeline_logs` | `PO ops/pipeline_runs.rs:25` |
| `jc_pipeline_metrics` | `pipeline_metrics` | `PO ops/views.rs:355` |
| `jc_pipeline_run` | `pipeline_run` | `PO connectors.rs:202` |
| `jc_pipeline_sample_source` | `pipeline_sample_source` | `PO ops/pipeline_steps.rs:1007` |
| `jc_pipeline_test` | `pipeline_test` | `PO ops/compute.rs:470` |
| `jc_pipeline_try_mapping` | `pipeline_try_mapping` | `PO ops/pipeline_steps.rs:1024` |
| `jc_pipeline_validate` | `pipeline_validate` | `PO ops/pipeline_steps.rs:1041` |
| `jc_project_create` | `project_create` | `PO ops/admin.rs:231` |
| `jc_project_delete` | `project_delete` | `PO ops/admin.rs:266` |
| `jc_project_export` | `project_export` | `PO ops/admin.rs:286` |
| `jc_project_get` | `project_get` | `PO ops/views.rs:631` |
| `jc_project_import` | `project_import` | `PO ops/admin.rs:335` |
| `jc_project_revisions` | `project_revisions` | `PO ops/views.rs:689` |
| `jc_resource_delete` | `set_delete` | `PO ops/resources.rs:693` |
| `jc_resource_get` | `set_get` | `PO ops/resources.rs:655` |
| `jc_resource_list` | `set_query` | `PO ops/resources.rs:636` |
| `jc_resource_propose` | `change_propose` | `PO ops/resources.rs:674` |
| `jc_run_answer` | `run_answer` | `PO ops/runs.rs:403` |
| `jc_run_cancel` | `run_cancel` | `PO ops/runs.rs:343` |
| `jc_run_create` | `run_create` | `PO connectors.rs:211` |
| `jc_run_get` | `run_get` | `PO ops/views.rs:574` |
| `jc_run_list` | `run_list` | `PO ops/views.rs:537` |
| `jc_run_message` | `run_message` | `PO ops/runs.rs:437` |
| `jc_run_publish` | `run_publish` | `PO ops/runs.rs:372` |
| `jc_schema_get` | `schema_get` | `PO ops/schemas.rs:737` |
| `jc_space_complete` | `space_complete` | `PO ops/space_complete.rs:2017` |
| `jc_space_governance` | `space_governance` | `PO ops/space_reads.rs:20` |
| `jc_syncsource_detach` | `syncsource_detach` | `PO ops/sync_sources.rs:195` |
| `jc_syncsource_pause` | `syncsource_pause` | `PO connectors.rs:206` |
| `jc_syncsource_status` | `syncsource_status` | `PO ops/sync_sources.rs:99` |
| `jc_syncsource_sync` | `syncsource_sync` | `PO ops/sync_sources.rs:136` |
| `jc_thing_write` | `thing_write` | `PO ops/mod.rs:905` |
| `jc_workspace_compare` | `workspace_compare` | `PO ops/workspaces.rs:1768` |
| `jc_workspace_discard` | `workspace_discard` | `PO ops/workspaces.rs:1839` |
| `jc_workspace_get` | `workspace_get` | `PO ops/workspaces.rs:1748` |
| `jc_workspace_list` | `workspace_list` | `PO ops/workspaces.rs:1729` |
| `jc_workspace_open` | `workspace_open` | `PO ops/workspaces.rs:1709` |
| `jc_workspace_preview_get` | `workspace_preview_get` | `PO ops/workspaces.rs:1883` |
| `jc_workspace_preview_start` | `workspace_preview_start` | `PO ops/workspaces.rs:1863` |
| `jc_workspace_preview_stop` | `workspace_preview_stop` | `PO ops/workspaces.rs:1903` |
| `jc_workspace_propose` | `workspace_propose` | `PO ops/workspaces.rs:1818` |
| `jc_workspace_update_from_main` | `workspace_update_from_main` | `PO ops/workspaces.rs:1788` |

The operations jc-portal-1 registered after the standalone Portal follow the same rule, today's name being the v1 name with `jc_` in front: `access_request_decide` (`PO ops/administration.rs:26`), `access_request_list` (`PO ops/administration.rs:25`), `organization_read` (`PO ops/administration.rs:24`), `repository_commit` (`PO ops/repositories.rs:32`), `repository_commits` (`PO ops/repositories.rs:31`), `repository_file` (`PO ops/repositories.rs:30`), `repository_get` (`PO ops/repositories.rs:28`), `repository_list` (`PO ops/repositories.rs:27`), `repository_pull` (`PO ops/repositories.rs:35`), `repository_pulls` (`PO ops/repositories.rs:34`), `repository_refs` (`PO ops/repositories.rs:33`), `repository_settings` (`PO ops/repositories.rs:36`), `repository_tree` (`PO ops/repositories.rs:29`), `serviceaccount_access` (`PO ops/administration.rs:27`).

## 6. Kept addresses

Prod (`smartcity.joinedcontext.com`, frozen 2026-11-09) published the addresses below in CKAN records, DCAT catalogues, calendar subscriptions, MCP connector settings and App code. Prod's hosts become host-root rows of §2 (`api.smartcity.joinedcontext.com/` maps to `/{org}/api/`, `opendata.` to `/{org}/data/`, `portal.` to `/{org}/`), so they use the same mechanism as an own domain; because each of them served several Organizations, the Organization comes from the row the path names (last column), never from the host. Each family stays served on the platform that replaces prod (EP-181):

- **In place** for API, feed, MCP and Git calls: the same status, headers and body as the v1 address, with canonical links in the body. No redirect, because calendar apps, MCP clients and scripts do not all follow `308`.
- **`308` to the v1 address** for a browser page.
- **No expiry** while the access log counts a call to the address in the last 12 months. Decided (supervisor, owner may override).

The counts are from prod's final export (`prod-final-20261109T1958Z`: 687 Endpoints, 9 Projects, 78 Context Spaces, 9 McpServers, 8 CkanInstances, 4 Organizations).

| Kept address | Count in the export | v1 address | Served | Organization from |
|---|---|---|---|---|
| `https://api.smartcity.joinedcontext.com/api/endpoint/{slug}/{surface}/...` | 687 slugs (26 or 32 characters, none shared); surfaces enabled: `ngsi-ld` 687, `mcp` 687, `ogc-features` 686, `odata` 659, `csv` 650, `websocket` 607, `ical` 8, `sta` 7 | `/{org}/api/endpoint/{slug}/{surface}/...` | in place | the Endpoint row of `{slug}` |
| `https://api.smartcity.joinedcontext.com/api/endpoint/{slug}/` record, `schema/...`, `openapi.json`, `access...`, `file.csv`, `ical/v1/...` | the same 687 | §3 | in place | the Endpoint row of `{slug}` |
| `https://api.smartcity.joinedcontext.com/cs/{space}/...` | 78 Context Spaces | `/{org}/cs/{space}/...` | in place | the Context Space row |
| `https://api.smartcity.joinedcontext.com/api/mcp/{project}/{name}` | 9 McpServers (`otvorene-data` in 6 Projects, `dades-obertes`, `kraj-a-mesto`, `open-data`) | `/{org}/api/mcp/{project}/{name}` | in place | the Project row |
| `https://api.smartcity.joinedcontext.com/api/mcp` (data hub) | 1 | `/{org}/api/mcp/data` | in place | signed in: the token's Organization; anonymous: a listing alias (below) |
| `https://api.smartcity.joinedcontext.com/catalog.jsonld`, `/catalog.ttl`, `/catalog/{file}` | 1 each, harvested | `/{org}/catalog...` | in place | a listing alias |
| `https://portal.smartcity.joinedcontext.com/api/v1/mcp` | 1 | `/{org}/api/mcp` | in place | the token's Organization |
| `https://portal.smartcity.joinedcontext.com/c/{id}` | one per connector | `/{org}/c/{id}` | in place | the connector row |
| `https://portal.smartcity.joinedcontext.com/api/v1/...` | 303 operations | `/{org}/api/v1/...` | in place | the Project row of `{project}`, else the token's Organization |
| `https://portal.smartcity.joinedcontext.com/projects/{project}/...` | 9 Projects | `/{org}/projects/{project}/...` | `308` | the Project row |
| `https://opendata.smartcity.joinedcontext.com/dataset/{name}`, `/dataset/{name}/resource/{id}`, `/{lang}/dataset/{name}` | 647 catalogued Endpoints in 8 CKAN organisations | `/{org}/data/dataset/{name}` | `308` | the dataset row of `{name}` (names are unique in the one prod catalogue) |
| `https://opendata.smartcity.joinedcontext.com/organization/{name}` | 8 | `/{org}/data/organization/{name}` | `308` | the publisher row |
| `https://opendata.smartcity.joinedcontext.com/api/3/action/...`, `/catalog...`, `/feeds/...`, `/odata/v4/...`, `/mcp` | 1 catalogue | `/{org}/data/...` | in place | a dataset or publisher row when the call names one, else a listing alias |
| `https://smartcity.joinedcontext.com/git/{owner}/{repo}.git` | 26 repositories of 2 owners (`joinedcontext` 10, `joinedcontext-apps` 16) | `/{org}/git/{owner}/{repo}.git` | in place | the repository row |
| `https://assistant.smartcity.joinedcontext.com/d/{project}`, `/api/v1/d` | 4 assistant pages | `/{org}/d/{project}`, `/{org}/api/v1/d` | page `308`, API in place | the Project row |
| `https://{app}.apps.smartcity.joinedcontext.com/` | 38 Apps | `{app}--{org}.{apps-base}` | `308` | the App row |
| `https://ai.smartcity.joinedcontext.com`, `https://data.smartcity.joinedcontext.com` | origins in 4 assistant allow-lists and 1 sign-in callback | the Organization's canonical host | `308` | the Project row of the manifest that names it |

A **listing alias** answers a call that names no single object over a whole prod catalogue (the anonymous hub, the DCAT and CKAN listings that a national portal harvests). It is read-only, serves public Endpoints and datasets only, and names in its own row the Organizations it covers; every item in the answer keeps the address of its own Organization. It never authenticates a write and never widens what a caller reads beyond the union of those Organizations' public data.

The issuer of prod's realm (`smartcity`) is not kept: tokens expire within 5 minutes, so a person signs in once on the new issuer and a client re-reads discovery.

## 7. Changing an address

- The route table is generated from the registries and the live Endpoint table, and a committed route golden lists every public path, surface and MCP tool name. CI fails on a path, surface or tool added, removed or renamed unless the golden and this page change in the same commit (EP-185).
- An addition needs only the golden and this page. A rename or a removal adds a kept-address row (`308` for a page, in place for API, feed, MCP and Git) and keeps the old name of a tool as an alias.
- A slug change, a host change and a move from a shared to a bound host keep the old addresses for at least 12 months. Ids do not change.
- The management API changes additively inside `/api/v1`; a removal or a type change ships as `/api/v2` beside v1 ([API Change Policy](10-api-change-policy.md)).

## 8. Security properties

- No address derives an Organization or a right from URN segments, host text or a header. The Organization comes from the first path segment through the registry, from the host row of a bound host, from the MQTT credential, or for a kept address from the one row the address names (Endpoint slug, Context Space, Project, McpServer, connector, dataset, publisher, repository, App) (EP-182).
- Every kept address that names an object is bound to exactly that object's Organization, and follows it when the object moves between Organizations. Only a listing alias covers more than one Organization, and only for anonymous public reads.
- An Organization id is minted only by an operator, from a domain whose ownership a DNS TXT record verified (EP-186).
- A kept address answers with the same authorization, Policy, limits and audit as its v1 address: it is a second path into the same handler, never a side path.
- Issuer, discovery, redirect URIs, cookie attributes, mail links and `@id` values are built from the registry's canonical host and the Organization id, never from request headers (EP-183).

## Related

- [API/02 Endpoint representations](02-endpoint-representations.md) — the wire format of each surface listed in §3.
- [API/10 Public API change policy](10-api-change-policy.md) — notice periods for a breaking change of a public Endpoint.
- [API/14 Connectors](14-connectors.md) — the connector record behind `/{org}/c/{id}` and its tool set.
- [Requirements/endpoints.md](../Requirements/endpoints.md#29-addresses-v1) — EP-180…EP-186, the requirements this page states.
- [Architecture/07 Agents and MCP](../Architecture/07-agents-and-mcp.md) — the MCP doors and how a client signs in.
