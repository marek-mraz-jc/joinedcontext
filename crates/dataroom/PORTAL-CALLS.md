# CKAN calls the Portal makes, and what dataroom answers

Inventory of 2026-11-08 (branch `jc-catalog-dash-portal`). Every call below is replayed by
`api/tests/portal_contract.rs` against `dataroom_api::mount("/opendata", ..)` over a loopback socket.
The writes go through the Portal's own client and publisher (`jcctl::publish::ckan_http::HttpCkan`
and `jcctl::publish::ckan::{publish, publish_views, withdraw}`), not a copy of their payloads.

Paths: `portal/` is `crates/portal/src/`, `jcctl/` is
`vendor/joinedcontext-platform/crates/jcctl/src/`.

## 1. Configuration

| Setting | Where | Meaning for dataroom |
|---|---|---|
| `JC_PORTAL_CKAN_URL` | `portal/config.rs:888` | Public URL a CkanInstance `spec.url` names; the Portal reads the catalogue anonymously there. |
| `JC_PORTAL_CKAN_API_URL` | `portal/config.rs:888` | In-cluster address the publisher writes to when `spec.url` equals `JC_PORTAL_CKAN_URL` (`portal/reconciler/ckan.rs:140` `reaching`, `:150` `api_url`). In `jc`: the dataroom mount, e.g. `http://127.0.0.1:8080/opendata`. |
| CkanInstance `apiTokenRef` | `portal/reconciler/ckan.rs:389` `publisher::token` | The token the publisher sends. It must equal the content of the file dataroom reads with `portal_publisher_from_file` (`DATAROOM_PORTAL_TOKEN_FILE` in `dataroom serve`). |

## 2. Writes (publisher, token in `Authorization`)

`HttpCkan` (`jcctl/publish/ckan_http.rs:108` `send`) sends the raw token in `Authorization`, reads
the `{success, result}` envelope, maps 404 to "not there" and 401/403 to a refusal.

| # | Call | Caller | Payload | What the Portal parses |
|---|---|---|---|---|
| W1 | `GET organization_show?id=<org>` | `jcctl/publish/ckan.rs:547` | | 404 or `result.title` |
| W2 | `POST organization_create` | `jcctl/publish/ckan.rs:553` | `{name, title}` | success |
| W3 | `POST organization_patch` | `jcctl/publish/ckan.rs:569` | `{id, title}` (declared instance title differs) | success |
| W4 | `GET package_show?id=<name>` | `jcctl/publish/ckan.rs:578`, `:698`, `:994` | | 404, or the package: `id`, `state`, every field it sent (`same`, `jcctl/publish/ckan.rs:1575`), `resources[].id`, `extras` |
| W5 | `POST package_create` | `jcctl/publish/ckan.rs:580` | `name, owner_org, title, notes, url, private, resources[{name,url,format,mimetype,description}], extras[{key,value}], tags[{name}]` | success |
| W6 | `POST package_update` | `jcctl/publish/ckan.rs:603` | W5 plus `id`, `state: "active"` after a withdrawal | success |
| W7 | `POST package_delete` | `jcctl/publish/ckan.rs:999` | `{id}` | success |
| W8 | `GET resource_view_list?id=<resource>` | `jcctl/publish/ckan.rs:746` | | `[{id, view_type, jc_name, title, description, spec, source, endpoint}]` |
| W9 | `POST resource_view_create` / `_update` / `_delete` | `jcctl/publish/ckan.rs:773`, `:762`, `:766` | `resource_id, view_type: jc_visualization, title, description, jc_name, spec, source, endpoint` (+`id`) | success |
| W10 | `POST package_search` | `portal/reconciler/ckan.rs:684` `sweep_with` | `{fq: organization:"<org>", include_private: true, rows, start}` | `result.count`, `results[].name`, `.organization.name`, `.extras[generated_by]` |

The Portal's calls go through `Paced` (`portal/reconciler/ckan.rs:631`), which caches
`organization_show` and adds no call of its own.

## 3. Anonymous reads

| # | Call | Caller | What the Portal parses |
|---|---|---|---|
| R1 | `GET package_search?q&rows&include_private=false` | `portal/api/catalogue.rs:920` `search` | `results[]`: `name, title, notes, private, license_*, metadata_modified, organization{name,title}, tags[{name,display_name}], resources[{format,url}]` (`slim`, `:960`) |
| R2 | `GET package_show?id=<name>` | `portal/api/catalogue.rs:1190` `dataset` | `success`; `private != true`, `name`, `extras[endpoint]` |
| R3 | `GET package_search?rows=1000&start&sort=name asc` | `portal/reconciler/catalog.rs:441` `packages` | `result.count`, `results[]` folded by jc-core `ngsi_catalog::Package::from_ckan` (`private == false`, `state` active, one Endpoint slug in `resources[].url`) |

Both readers refuse a CkanInstance URL naming an internal address (`portal/api/catalogue.rs:869`,
`portal/reconciler/catalog.rs:431`): the public URL in `JC_PORTAL_CKAN_URL` must be a public
host name that reaches the dataroom mount, never `127.0.0.1`.

## 4. Visibility (EP-139)

- A public Endpoint's dataset (`private: false`) is answered to R1, R2 and R3.
- A closed Endpoint's dataset (`private: true`) is answered only to the publisher's token (W4, W10).
- An Endpoint that stops being public is updated to `private: true` and leaves R1/R2 at once.
- A withdrawn dataset (W7) is gone from R1, R2 and R3; W4 still shows it as `state: deleted`, so a
  second withdraw is a no-op and a republish is a `package_update` with `state: active`.

## 5. The Portal's binding in dataroom

`dataroom_api::portal_publisher(_from_file)` binds the token (at least 32 printable characters,
never printed: `TokenBinding`'s `Debug` redacts it) to principal `portal-ckan` with one grant:
`Read` and `Propose` on the catalogue. That is all W1 to W10 need: `package_delete` is a
withdrawal (`dataset.withdraw`), not a purge, so `Delete` and `Admin` stay out. Red proofs: the
contract test fails with `Read` alone.

## 6. Gaps for the `jc` wiring (not in these paths)

- `bins/jc/src/modules/dataroom.rs` builds `PolicyAuthorizer::default()` with no token, so every
  Portal write is refused 401. Wire it as `dataroom serve` does (`dataroom_server::serve_state`):
  `portal_publisher_from_file(<secret file>, org, catalogue)`, the grant into `PolicyAuthorizer::new`,
  the binding into `with_tokens`.
- The same secret has to reach the Portal as the CkanInstance `apiTokenRef` (forge SOPS secret).
- `jc` serves dataroom from a memory store: published datasets are lost on restart until the
  Postgres store is wired (bins/jc/STATUS.md).
