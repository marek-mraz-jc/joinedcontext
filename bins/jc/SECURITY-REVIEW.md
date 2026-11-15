# Security review of the generated modules

Review of `crates/{kaisatsu,shako,ekiben,dataroom,daiya}` and `bins/jc`, 2026-11-08, by jc-rest.
One Claude subagent per crate read the code for: authorization deny-by-default, tenant isolation,
injection, SSRF and the outbound guard, secrets in logs, crypto, `unsafe`, and DoS limits. Each
finding carries one of three verdicts:

- **fixed**: a guard landed, with a test that goes red when the guard is reverted (the red run is
  recorded in the body of the task that landed it).
- **documented**: not fixed here; the row names the fix and where it goes.
- **latent**: not reachable in `jc` today; the row names what would make it reachable.

Every row's `Test` cell names the test that proves it, or `none:` with the reason (an open fix, a
latent path, a design note); `.github/scripts/security-review-tests.py` fails CI when a named test
does not exist. No `unsafe` was found in any reviewed crate. Every high finding is fixed (J-1 in the deployment repository).

## 1. Fixed

| Id | Sev | What was wrong | Guard | Test |
|---|---|---|---|---|
| K-1 | high | group create/update kept `realmRoles`/`clientRoles` from the body | `guard_group_write` keeps the stored roles (empty on create); roles change only through the role-mapping routes | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-2 | high | an org admin could set `jc_org`/`jc_groups` on a group | an organization-scoped caller cannot change those attributes (403) | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-3 | high | group create could replace another group by body `id`/`path`/`subGroups` | create ignores id, parent and sub-groups and computes the path from the name; a taken top-level name answers 409; update keeps the stored path | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-4 | high | `POST /clients` replaced a client by `clientId` or `id` | a taken clientId or id answers 409; a rename onto a taken clientId answers 409 | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-5 | high | `POST /client-scopes` replaced a scope by body `id` | a taken id answers 409 | kaisatsu-admin `compat_routes::writes_cannot_take_over_other_objects_or_grant_through_group_bodies` |
| K-6 | high | protocol mappers could write `sub`, `iss`, `aud`, `realm_access`, `jc_org`, ... | `mapper_may_write`: reserved claims never, authority claims only on the mapper's own native path | kaisatsu-core `claims::tests::mappers_cannot_write_reserved_or_authority_claims`, `a_client_mapper_mints_no_role_organization_or_subject` |
| S-2 | high | a few-KB tree bomb pushed to shako overflowed the stack or memory of the whole process | `collect_tree_blobs` refuses depth > 256 and > 500 000 entries; the push answers the reason | shako-git `repo::tests::a_tree_bomb_is_refused_before_it_fills_memory_or_the_stack` |
| E-1 | high | NATS components dialled any address and followed server-advertised addresses | every URL passes `resolve_host_checked` (the shared outbound rule); plain connections dial the checked IP; `ignore_discovered_servers` | ekiben-io-nats `connection::tests::server_addresses_pass_the_outbound_rule` |
| E-2 | high | NATS `*_file` options read any path | each one goes through `sanitize_file_path` under `file_root` | ekiben-io-nats `connection::tests::file_options_stay_under_the_file_root` |
| D-1 | high | a recursive SHACL shape recursed without a limit (anonymous `validate-sample`) | `check_references`: an iterative cycle and depth check (32) before compile and validation | dataroom-shacl `recursive_or_too_deep_shapes_are_refused_before_validation` |
| D-2 | high | `dataset.search` returned publish-gate hidden datasets | hidden datasets skipped without Propose | dataroom-api `routes::anonymous_listings_hold_no_draft_hidden_or_withdrawn_dataset` |
| D-3 | high | `catalogue.get`/`list` returned `hidden_datasets` and `allowed_outbound_hosts` | `public_catalogue` clears both without Propose on the catalogue | dataroom-api `routes::anonymous_listings_hold_no_draft_hidden_or_withdrawn_dataset` |
| D-4 | high | Atom feed path parameter reflected unescaped into XML | addresses escaped; `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'` | dataroom-api `routes::feed_addresses_are_escaped_and_feeds_run_no_script` |
| D-5 | high | anonymous `package_search` with `include_drafts`/`include_private` returned drafts and withdrawn publishers | the search skips non-Active datasets and withdrawn publishers for anonymous callers | dataroom-api `routes::anonymous_listings_hold_no_draft_hidden_or_withdrawn_dataset`, dataroom-search `tests::search_filters_sorts_pages_and_keeps_active_facet_values` |
| D-19 | low | `(page - 1) * 20` overflowed | `saturating_mul` in `dataset.search`, `/api/v1/catalogue` and `/dataset` | dataroom-api `routes::a_huge_page_number_answers_an_empty_page` |
| Y-1 | high | SQL checker skipped quoted function names and `TABLE rel` | a quoted name followed by `(` and any `TABLE` statement are refused | daiya-query `postgres::the_checker_refuses_malformed_and_foreign_sql_with_a_reason` |
| Y-2 | high | an Editor created or repointed datasources (url, secrets, allowed views) | datasource create and update need Admin on the Organization | daiya-api `compat_routes::datasource_crud_answers_as_grafana` (editor POST/PUT 403) |
| Y-4 | medium | public-dashboard range cap bypassed by i64 overflow | saturating range and interval arithmetic | daiya-api `public_routes::an_overflowing_public_range_is_still_capped` |
| J-1 | high | the deployed `jc.toml` had no `trusted_proxies`: every caller was the APISIX pod, so per-address sign-in locks and rate limits were one bucket | deployment `jc-ship` 92fcc9b4e: `trusted_proxies` from the environment's `jc.server.trustedProxies` (addons: 10.42.0.0/16); the render refuses an empty list and any /0 | repo:joinedcontext-deployment tests/test_jc_component.py |
| Y-10 | low | `intervalFactor` multiplied the step unchecked | `saturating_mul` | daiya-query `prometheus::steps_round_like_grafana` |

## 2. Documented: medium

| Id | Fix | Test |
|---|---|---|
| K-7 | refuse person update, delete and credential reset by an org role holder when the target holds a realm-level admin role (`authz.rs` person target carries the target's realm roles) | kaisatsu-admin `authz_regressions::org_admin_cannot_reset_or_change_a_realm_admin_of_the_organization` |
| K-8 | judge each object of `partialImport` and `PUT /admin/realms/{realm}` with the authorizer as the single-object routes do, and refuse literal client secrets there | none: open, the fix is not built |
| S-1 | run the branch-protection check in PR update-branch (`compat.rs:2814`), OData CreateBranch (`odata.rs:1733`) and delete-after-merge (`compat.rs:3064`) | none: open (CreateBranch is checked; update-branch and delete-after-merge are not) |
| S-3 | size the receive-pack permit to the bytes buffered (or stream to a temp pack); registry uploads take a permit, count toward the storage quota, and the session map gets a cap and expiry | shako-registry `tests::open_uploads_are_capped_in_number_and_bytes_and_expire`; none: the receive-pack permit sized to the bytes buffered is open |
| S-4 | bind a mirror or webhook `secret_ref` to its repository (name prefix `repo/<id>/`), refuse others | none: open, the fix is not built |
| S-5 | persist the owner-to-org map in the store and load it at start; refuse an owner name another organization holds | none: open, the fix is not built |
| S-6 | refuse a path starting `//` or `/%2F` and a `redirect_to` that is not a same-origin path after percent-decoding and whitespace stripping | shako-web `browser_to_portal::the_redirect_lands_on_the_portal_base_and_never_off_it`, `tests::a_page_path_names_the_portal_page_of_its_repository_and_nothing_else` |
| S-7 | team and permission no-op routes check membership of the org in the path before writing outbox events | none: open, the fix is not built |
| E-3 | jc sets streams mode for ekiben, so `http_server` inputs bind only jc's ingest route, never their own listener | none: open, the fix is not built |
| E-4 | cap Bloblang method-chain length at parse (as the SHACL depth check does) and drop the `Expr` tree iteratively | none: open, the fix is not built |
| D-6 | dataset HTML and RDF pages check `state == Active` for anonymous callers, as D-5's search now does | none: open, the fix is not built |
| D-7 | run anonymous SHACL validation in `spawn_blocking` with a work budget (focus x shapes) | none: open (validation runs in `spawn_blocking`; the work budget is not built) |
| D-8 | `catalogue.propose` checks Admin for `branding` and Approve for profiles, `hidden_datasets` and `allowed_outbound_hosts` | dataroom-core `ops_registry::catalogue_propose_without_admin_cannot_edit_branding_hosts_or_outbound_allowlist` |
| D-9 | `organization_create` ignores the payload `id` and answers 409 on a taken name or id | dataroom-api `routes::ckan_organization_writes`, `integration::two_publishers_in_one_catalogue_with_separate_editor_grants_and_merge` |
| D-10 | the memory store keeps distribution ids unique per catalogue | none: open, the fix is not built |
| D-11 | `Cache-Control: private` and `Vary: Authorization, Cookie` on authenticated catalogue answers | dataroom-api `routes::portal_catalogue_and_dataset_answer_like_package_show`; none: `Vary` on `/api/v1/catalogue` is open (the CKAN routes send it) |
| D-12 | `GET /api/v1/profiles` lists built-in profiles and the caller's tenant's own, not the process registry (known open H4) | dataroom-api `request_origin::a_catalogues_uploaded_profile_is_listed_only_on_its_own_host` |
| D-13 | `package_list`, `tag_list`, `organization_show` counts, `catalogue.feed_check` and the home page leave out hidden datasets and withdrawn publishers | none: open, the fix is not built |
| Y-3 | byte cap on Postgres results beside the row cap (sum of cell sizes) | none: open, the fix is not built |
| J-2 | mount shako's `/v2` only on the shako host in host mode, or add `/v2` to the edge `uri-blocker` in slug mode | none: open, the fix is not built |
| J-3 | siblings reach each other through jc's own listener (`127.0.0.1:<port>`) in host mode too (`siblings.rs` builds the internal URL from jc's listen address, not the public mount) | jc `siblings::tests::host_mode_hands_the_portal_every_sibling_on_the_own_listener` |
| J-4 | host mode (one host per module) is the cutover layout (`CUTOVER.md` section 4); slug mode stays for a single-tenant install only | none: a layout decision, written in `CUTOVER.md` section 4 |

## 3. Documented: low and latent

| Id | Verdict | Fix | Test |
|---|---|---|---|
| K-9 | fixed | kaisatsu's outbound guard exempts only the exact Kubernetes API host (`https`, port 443) and the exact hosts listed in `[kaisatsu] outbound_private_hosts` (no suffix rule, no address literal); the ServiceAccount token goes to the Kubernetes API alone; `backchannel.logout.url` and identity-provider `jwksUrl`/`tokenUrl` are checked on save (422 naming the field), and a realm import keeps a refused stored value with a warning while every send to it is refused | kaisatsu-oidc `guard::tests::a_cluster_jwks_is_read_from_the_kubernetes_api_only`, `only_a_listed_exact_host_skips_the_public_address_rule`; kaisatsu-admin `compat_tests::a_client_sign_out_address_the_outbound_guard_refuses_is_refused_on_save` |
| K-10 | documented | first broker login asks the existing account to sign in (or confirm by e-mail) before linking, whatever credentials it holds | kaisatsu-server `broker_flow::k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation`; none: the account without a password is open |
| S-8 | documented | a project binding matches only targets in that project; repo bindings match owner and repo; principals keep their kind | shako-core `auth::tests::a_project_binding_grants_on_that_projects_repositories_alone`; none: repository bindings and principal kinds are open |
| S-9 | documented | filter the OData sets `Deliveries`, `Events`, `Packages*` to visible repositories and projects | none: open (`Deliveries` is filtered; `Events` and `Packages*` are not) |
| S-10 | documented | queue webhook and mirror work after the request, outside the repo lock | none: open, the fix is not built |
| E-5 | fixed | the guarded `http_client` judges every address by the shared outbound rule (NAT64, 6to4, IPv4-compatible), sends an Organization's listed `private_hosts` through a client that admits private ranges only (never loopback, link-local or metadata) and takes no proxy from the environment | ekiben-io-http `http_tests::a_listed_private_host_is_resolved_by_the_private_host_rule_and_never_reaches_loopback`, `http_tests::ssrf_and_scheme_security_guards` |
| E-6 | documented | redact `tls.client_certs[].key` in `GET /streams/{id}` | none: open, the fix is not built |
| E-7 | documented | check the nearest existing ancestor when the target does not exist, and recheck each glob match | none: open, the fix is not built |
| D-14 | documented | CKAN resource writes go through `dataset.propose` (publish gate, activity row) | none: open, the fix is not built |
| D-15 | documented | a Harvest grant allows starting a run only | none: open, the fix is not built |
| D-16 | documented | `publisher.list` and `plugin.list` hide editor grants, e-mail addresses and plugin config from callers without Admin | dataroom-core `ops_registry::plugins_profiles_branding_activity_and_feed_check`; none: `publisher.list` is open |
| D-17 | documented | escape `</untrusted-data` case-insensitively and with any trailing whitespace | dataroom-mcp `jsonrpc::tool_calls_run_the_registry_and_fence_untrusted_text`, `jsonrpc::the_fence_wraps_any_value` |
| D-18 | documented | `organization_update` ignores a payload `instance_name` | none: open, the fix is not built |
| D-20 | documented | cap logo uploads per publisher (one current logo, replaced) | none: open, the fix is not built |
| D-21 | latent | reachable only with a cookie `SubjectResolver`; then CKAN POST must demand `application/json` | none: latent, jc sets no cookie `SubjectResolver` |
| Y-5 | documented | import ignores a caller's `isProvisioned` | none: open, the fix is not built |
| Y-6 | documented | the public view strips `templating` and `annotations` query text when `showQueryText` is false | none: open, the fix is not built |
| Y-7 | documented | MCP public queries rate-limit by the MCP caller's identity; the per-address table evicts the oldest entry when full | daiya-api `public_routes::the_limiter_resets_after_its_window_and_bounds_its_table`; none: the per-caller MCP limit and oldest-entry eviction are open |
| Y-8 | latent | with a real token exchanger (jc-rest item 5, not built) the exchange audience must come from server config, never from the datasource record | none: latent, jc has no token exchanger that takes the audience from a datasource |
| Y-9 | documented | datasource delete answers 403 for `readOnly` datasources, as Grafana does | none: open, the fix is not built |
| J-5 | documented | a connection cap, header-read and request timeouts and a global body limit on jc's public listener | none: open, the fix is not built |
| J-6 | documented | a shutdown deadline, and module background tasks on the TaskTracker | none: open, the fix is not built |
| J-7 | documented | read the two database URLs from files (`*_FILE`) as the other secrets are | none: open, the fix is not built |
| J-8 | documented | default roots under a jc-owned `0700` directory, refuse an existing path not owned by jc | jc `modules::ekiben::tests::file_root_is_created_private`; none: the jc-owned default root and the owner check are open |
| J-9 | documented | write secret files to a temp file and rename | none: open, the fix is not built |
| J-10 | documented | include shako's host-root `/v2` mount in `plan_mounts`' overlap dry run | none: open, the fix is not built |

## Related

- `evidence/jc-rest/review/*.md`: the full per-crate reports, with exploit paths (not for public notes).
- `bins/jc/STATUS.md`, `bins/jc/CUTOVER.md`.
