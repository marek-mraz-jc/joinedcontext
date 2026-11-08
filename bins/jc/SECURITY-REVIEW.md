# Security review of the generated modules

Review of `crates/{kaisatsu,shako,ekiben,dataroom,daiya}` and `bins/jc`, 2026-11-08, by jc-rest.
One Claude subagent per crate read the code for: authorization deny-by-default, tenant isolation,
injection, SSRF and the outbound guard, secrets in logs, crypto, `unsafe`, and DoS limits. Each
finding carries one of three verdicts:

- **fixed**: a guard landed on branch `jc-rest`, with a test that goes red when the guard is reverted
  (the red runs are listed in `evidence/jc-rest/redproofs.txt`).
- **documented**: not fixed here; the row names the fix and where it goes.
- **latent**: not reachable in `jc` today; the row names what would make it reachable.

No `unsafe` was found in any reviewed crate. Every high finding is fixed except J-1, which lives in
the deployment repository.

## 1. Fixed

| Id | Sev | What was wrong | Guard | Test |
|---|---|---|---|---|
| K-1 | high | group create/update kept `realmRoles`/`clientRoles` from the body | `guard_group_write` keeps the stored roles (empty on create); roles change only through the role-mapping routes | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-2 | high | an org admin could set `jc_org`/`jc_groups` on a group | an organization-scoped caller cannot change those attributes (403) | same |
| K-3 | high | group create could replace another group by body `id`/`path`/`subGroups` | create ignores id, parent and sub-groups and computes the path from the name; a taken top-level name answers 409; update keeps the stored path | same |
| K-4 | high | `POST /clients` replaced a client by `clientId` or `id` | a taken clientId or id answers 409; a rename onto a taken clientId answers 409 | same |
| K-5 | high | `POST /client-scopes` replaced a scope by body `id` | a taken id answers 409 | same |
| K-6 | high | protocol mappers could write `sub`, `iss`, `aud`, `realm_access`, `jc_org`, ... | `mapper_may_write`: reserved claims never, authority claims only on the mapper's own native path | kaisatsu-core `claims::tests::mappers_cannot_write_reserved_or_authority_claims`, `a_client_mapper_mints_no_role_organization_or_subject` |
| S-2 | high | a few-KB tree bomb pushed to shako overflowed the stack or memory of the whole process | `collect_tree_blobs` refuses depth > 256 and > 500 000 entries; the push answers the reason | shako-git `repo::tests::a_tree_bomb_is_refused_before_it_fills_memory_or_the_stack` |
| E-1 | high | NATS components dialled any address and followed server-advertised addresses | every URL passes `resolve_host_checked` (the shared outbound rule); plain connections dial the checked IP; `ignore_discovered_servers` | ekiben-io-nats `connection::tests::server_addresses_pass_the_outbound_rule` |
| E-2 | high | NATS `*_file` options read any path | each one goes through `sanitize_file_path` under `file_root` | ekiben-io-nats `connection::tests::file_options_stay_under_the_file_root` |
| D-1 | high | a recursive SHACL shape recursed without a limit (anonymous `validate-sample`) | `check_references`: an iterative cycle and depth check (32) before compile and validation | dataroom-shacl `recursive_or_too_deep_shapes_are_refused_before_validation` |
| D-2 | high | `dataset.search` returned publish-gate hidden datasets | hidden datasets skipped without Propose | dataroom-api `routes::anonymous_listings_hold_no_draft_hidden_or_withdrawn_dataset` |
| D-3 | high | `catalogue.get`/`list` returned `hidden_datasets` and `allowed_outbound_hosts` | `public_catalogue` clears both without Propose on the catalogue | same |
| D-4 | high | Atom feed path parameter reflected unescaped into XML | addresses escaped; `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'` | dataroom-api `routes::feed_addresses_are_escaped_and_feeds_run_no_script` |
| D-5 | high | anonymous `package_search` with `include_drafts`/`include_private` returned drafts and withdrawn publishers | the search skips non-Active datasets and withdrawn publishers for anonymous callers | dataroom-api `routes::anonymous_listings_...`, dataroom-search unit test |
| D-19 | low | `(page - 1) * 20` overflowed | `saturating_mul` in `dataset.search`, `/api/v1/catalogue` and `/dataset` | dataroom-api `routes::a_huge_page_number_answers_an_empty_page` |
| Y-1 | high | SQL checker skipped quoted function names and `TABLE rel` | a quoted name followed by `(` and any `TABLE` statement are refused | daiya-query `postgres::the_checker_refuses_malformed_and_foreign_sql_with_a_reason` |
| Y-2 | high | an Editor created or repointed datasources (url, secrets, allowed views) | datasource create and update need Admin on the Organization | daiya-api `compat_routes::datasource_crud_answers_as_grafana` (editor POST/PUT 403) |
| Y-4 | medium | public-dashboard range cap bypassed by i64 overflow | saturating range and interval arithmetic | daiya-api `public_routes::an_overflowing_public_range_is_still_capped` |
| Y-10 | low | `intervalFactor` multiplied the step unchecked | `saturating_mul` | daiya-query `prometheus::steps_round_like_grafana` |

## 2. Documented: high

| Id | Where the fix goes | Fix |
|---|---|---|
| J-1 | deployment `components/jc/values/server/base-values.yaml.gotmpl` (`jc.toml`) | add `trusted_proxies` with the APISIX pod CIDR (the cluster pod network, `10.42.0.0/16` on k3s), so `jc-router` takes the client address from `X-Forwarded-For`; until then kaisatsu's per-address sign-in lock and daiya's public rate limit are one bucket for everybody. Not changed here: jc-rest pushes only the monorepo branch, and the cutover (`CUTOVER.md` step 0) is the place for the deployment change |

## 3. Documented: medium

| Id | Fix |
|---|---|
| K-7 | refuse person update, delete and credential reset by an org role holder when the target holds a realm-level admin role (`authz.rs` person target carries the target's realm roles) |
| K-8 | judge each object of `partialImport` and `PUT /admin/realms/{realm}` with the authorizer as the single-object routes do, and refuse literal client secrets there |
| S-1 | run the branch-protection check in PR update-branch (`compat.rs:2814`), OData CreateBranch (`odata.rs:1733`) and delete-after-merge (`compat.rs:3064`) |
| S-3 | size the receive-pack permit to the bytes buffered (or stream to a temp pack); registry uploads take a permit, count toward the storage quota, and the session map gets a cap and expiry |
| S-4 | bind a mirror or webhook `secret_ref` to its repository (name prefix `repo/<id>/`), refuse others |
| S-5 | persist the owner-to-org map in the store and load it at start; refuse an owner name another organization holds |
| S-6 | refuse a path starting `//` or `/%2F` and a `redirect_to` that is not a same-origin path after percent-decoding and whitespace stripping |
| S-7 | team and permission no-op routes check membership of the org in the path before writing outbox events |
| E-3 | jc sets streams mode for ekiben, so `http_server` inputs bind only jc's ingest route, never their own listener |
| E-4 | cap Bloblang method-chain length at parse (as the SHACL depth check does) and drop the `Expr` tree iteratively |
| D-6 | dataset HTML and RDF pages check `state == Active` for anonymous callers, as D-5's search now does |
| D-7 | run anonymous SHACL validation in `spawn_blocking` with a work budget (focus x shapes) |
| D-8 | `catalogue.propose` checks Admin for `branding` and Approve for profiles, `hidden_datasets` and `allowed_outbound_hosts` |
| D-9 | `organization_create` ignores the payload `id` and answers 409 on a taken name or id |
| D-10 | the memory store keeps distribution ids unique per catalogue |
| D-11 | `Cache-Control: private` and `Vary: Authorization, Cookie` on authenticated catalogue answers |
| D-12 | `GET /api/v1/profiles` lists built-in profiles and the caller's tenant's own, not the process registry (known open H4) |
| D-13 | `package_list`, `tag_list`, `organization_show` counts, `catalogue.feed_check` and the home page leave out hidden datasets and withdrawn publishers |
| Y-3 | byte cap on Postgres results beside the row cap (sum of cell sizes) |
| J-2 | mount shako's `/v2` only on the shako host in host mode, or add `/v2` to the edge `uri-blocker` in slug mode |
| J-3 | siblings reach each other through jc's own listener (`127.0.0.1:<port>`) in host mode too (`siblings.rs` builds the internal URL from jc's listen address, not the public mount) |
| J-4 | host mode (one host per module) is the cutover layout (`CUTOVER.md` section 4); slug mode stays for a single-tenant install only |

## 4. Documented: low and latent

| Id | Verdict | Fix |
|---|---|---|
| K-9 | documented | the guard's `*.svc.cluster.local` exemption applies only to configured realm-admin targets, never to client-controlled `backchannel.logout.url` |
| K-10 | documented | first broker login asks the existing account to sign in (or confirm by e-mail) before linking, whatever credentials it holds |
| S-8 | documented | a project binding matches only targets in that project; repo bindings match owner and repo; principals keep their kind |
| S-9 | documented | filter the OData sets `Deliveries`, `Events`, `Packages*` to visible repositories and projects |
| S-10 | documented | queue webhook and mirror work after the request, outside the repo lock |
| E-5 | documented | `http_client` uses the shared outbound rule (NAT64, 6to4, IPv4-compatible) and ignores `*_PROXY` |
| E-6 | documented | redact `tls.client_certs[].key` in `GET /streams/{id}` |
| E-7 | documented | check the nearest existing ancestor when the target does not exist, and recheck each glob match |
| D-14 | documented | CKAN resource writes go through `dataset.propose` (publish gate, activity row) |
| D-15 | documented | a Harvest grant allows starting a run only |
| D-16 | documented | `publisher.list` and `plugin.list` hide editor grants, e-mail addresses and plugin config from callers without Admin |
| D-17 | documented | escape `</untrusted-data` case-insensitively and with any trailing whitespace |
| D-18 | documented | `organization_update` ignores a payload `instance_name` |
| D-20 | documented | cap logo uploads per publisher (one current logo, replaced) |
| D-21 | latent | reachable only with a cookie `SubjectResolver`; then CKAN POST must demand `application/json` |
| Y-5 | documented | import ignores a caller's `isProvisioned` |
| Y-6 | documented | the public view strips `templating` and `annotations` query text when `showQueryText` is false |
| Y-7 | documented | MCP public queries rate-limit by the MCP caller's identity; the per-address table evicts the oldest entry when full |
| Y-8 | latent | with a real token exchanger (jc-rest item 5, not built) the exchange audience must come from server config, never from the datasource record |
| Y-9 | documented | datasource delete answers 403 for `readOnly` datasources, as Grafana does |
| J-5 | documented | a connection cap, header-read and request timeouts and a global body limit on jc's public listener |
| J-6 | documented | a shutdown deadline, and module background tasks on the TaskTracker |
| J-7 | documented | read the two database URLs from files (`*_FILE`) as the other secrets are |
| J-8 | documented | default roots under a jc-owned `0700` directory, refuse an existing path not owned by jc |
| J-9 | documented | write secret files to a temp file and rename |
| J-10 | documented | include shako's host-root `/v2` mount in `plan_mounts`' overlap dry run |

## Related

- `evidence/jc-rest/review/*.md`: the full per-crate reports, with exploit paths (not for public notes).
- `evidence/jc-rest/redproofs.txt`: each fixed guard reverted alone and the test that went red.
- `bins/jc/STATUS.md`, `bins/jc/CUTOVER.md`.
