# kaisatsu Keycloak Admin REST Compatibility Table (W1)

Every `/admin/realms/{realm}/...` call made by the Portal (`joinedcontext-portal`), deployment Jobs
(`joinedcontext-deployment`), and `keycloak-config-cli`, with its caller `file:line`, status codes,
query parameters, and test coverage in `crates/kaisatsu/admin/tests/compat_tests.rs`.
[PORTAL-CALLS.md](PORTAL-CALLS.md) lists the Portal's calls (OIDC and admin) with the identity
each uses and the answer it parses, replayed by `server/tests/portal_contract.rs`.

| # | Method | Path | Query Parameters | Status | Caller `file:line` | Test in `compat_tests.rs` |
|---|---|---|---|---|---|---|
| 1 | `GET` | `/admin/realms/{realm}` | `fields` | `200`, `404` | `charts/people/files/ensure-user-ids.sh:34` | `row_01_02_03_realms_and_partial_import` |
| 2 | `POST` | `/admin/realms` | — | `201`, `409` | `charts/people/files/ensure-user-ids.sh:35`, `keycloak-config-cli` | `row_01_02_03_realms_and_partial_import` |
| 3 | `PUT` | `/admin/realms/{realm}` | — | `204` | `keycloak-config-cli` | `row_01_02_03_realms_and_partial_import` |
| 4 | `POST` | `/admin/realms/{realm}/partialImport` | — (`ifResourceExists`: `FAIL`/`SKIP`/`OVERWRITE`) | `200`, `409` | `charts/people/files/ensure-user-ids.sh:99` | `row_01_02_03_realms_and_partial_import` |
| 5 | `GET` | `/admin/realms/{realm}/users` | `search`, `first`, `max`, `briefRepresentation`, `email`, `username`, `exact` | `200` | `POR src/people.rs:285`, `:332`, `src/reconciler/app_clients.rs:584`, `charts/people/files/ensure-user-ids.sh:45` | `row_05_to_17_users_lifecycle_and_subresources` |
| 6 | `POST` | `/admin/realms/{realm}/users` | — | `201 + Location`, `409` | `POR src/people.rs:319` | `row_05_to_17_users_lifecycle_and_subresources` |
| 7 | `GET` | `/admin/realms/{realm}/users/{id}` | — | `200`, `404` | `POR src/people.rs:298`, `src/reconciler/groups.rs:475`, `src/reconciler/app_clients.rs:572` | `row_05_to_17_users_lifecycle_and_subresources` |
| 8 | `PUT` | `/admin/realms/{realm}/users/{id}` | — | `204`, `404`, `409` | `POR src/people.rs:351` | `row_05_to_17_users_lifecycle_and_subresources` |
| 9 | `DELETE` | `/admin/realms/{realm}/users/{id}` | — | `204`, `404` | `POR src/people.rs:515` | `row_05_to_17_users_lifecycle_and_subresources` |
| 10 | `PUT` | `/admin/realms/{realm}/users/{id}/execute-actions-email` | `lifespan`, `client_id`, `redirect_uri` (validated against `client_id`'s registered `redirectUris`; atomically updates `requiredActions` and queues an `EmailOutboxRecord`, K9/K10) | `204`, `400`, `404`, `501` | `POR src/people.rs:373` | `row_05_to_17_users_lifecycle_and_subresources`, `k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys` |
| 11 | `PUT` | `/admin/realms/{realm}/users/{id}/reset-password` | — | `204` | `POR src/people.rs:395` | `row_05_to_17_users_lifecycle_and_subresources` |
| 12 | `POST` | `/admin/realms/{realm}/users/{id}/logout` | — | `204` | `POR src/people.rs:407` | `row_05_to_17_users_lifecycle_and_subresources` |
| 13 | `GET` | `/admin/realms/{realm}/users/{id}/credentials` | — | `200` | `POR src/people.rs:415` | `row_05_to_17_users_lifecycle_and_subresources` |
| 14 | `DELETE` | `/admin/realms/{realm}/users/{id}/credentials/{cred_id}` | — | `204` | `POR src/people.rs:423` | `row_05_to_17_users_lifecycle_and_subresources` |
| 15 | `GET` | `/admin/realms/{realm}/users/{id}/sessions` | — | `200` | `POR src/people.rs:434` | `row_05_to_17_users_lifecycle_and_subresources` |
| 16 | `GET` | `/admin/realms/{realm}/users/{id}/consents` | — | `200` | `POR src/people.rs:442` | `row_05_to_17_users_lifecycle_and_subresources` |
| 17 | `DELETE` | `/admin/realms/{realm}/users/{id}/consents/{client}` | — | `204` | `POR src/people.rs:451` | `row_05_to_17_users_lifecycle_and_subresources` |
| 18 | `GET` | `/admin/realms/{realm}/users/{id}/groups` | — | `200` | `POR src/people.rs:475`, `:490` | `row_18_to_26_groups_and_memberships` |
| 19 | `PUT` | `/admin/realms/{realm}/users/{id}/groups/{group_id}` | — | `204` | `POR src/people.rs:508`, `src/reconciler/groups.rs:384` | `row_18_to_26_groups_and_memberships` |
| 20 | `DELETE` | `/admin/realms/{realm}/users/{id}/groups/{group_id}` | — | `204` | `POR src/reconciler/groups.rs:408` | `row_18_to_26_groups_and_memberships` |
| 21 | `GET` | `/admin/realms/{realm}/users/{id}/role-mappings/realm/composite` | — | `200` | `POR src/people.rs:477` | `row_27_to_34_roles_and_role_mappings` |
| 22 | `GET` | `/admin/realms/{realm}/groups` | `search`, `exact`, `briefRepresentation`, `max` | `200` | `POR src/people.rs:503`, `src/reconciler/groups.rs:202`, `:330`, `src/reconciler/app_clients.rs:549` | `row_18_to_26_groups_and_memberships` |
| 23 | `POST` | `/admin/realms/{realm}/groups` | — | `201 + Location`, `409` | `POR src/reconciler/groups.rs:317` | `row_18_to_26_groups_and_memberships` |
| 24 | `PUT` | `/admin/realms/{realm}/groups/{id}` | — | `204` | `POR src/reconciler/groups.rs:459` | `row_18_to_26_groups_and_memberships` |
| 25 | `DELETE` | `/admin/realms/{realm}/groups/{id}` | — | `204` | `POR src/reconciler/groups.rs:281` | `row_18_to_26_groups_and_memberships` |
| 26 | `GET` | `/admin/realms/{realm}/groups/{id}/members` | `max` | `200` | `POR src/reconciler/groups.rs:367` | `row_18_to_26_groups_and_memberships` |
| 27 | `POST` | `/admin/realms/{realm}/users/{id}/role-mappings/clients/{client_uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:734` | `row_27_to_34_roles_and_role_mappings` |
| 28 | `DELETE` | `/admin/realms/{realm}/users/{id}/role-mappings/clients/{client_uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:703` | `row_27_to_34_roles_and_role_mappings` |
| 29 | `POST` | `/admin/realms/{realm}/groups/{id}/role-mappings/clients/{client_uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:734` | `row_27_to_34_roles_and_role_mappings` |
| 30 | `DELETE` | `/admin/realms/{realm}/groups/{id}/role-mappings/clients/{client_uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:684` | `row_27_to_34_roles_and_role_mappings` |
| 31 | `GET` | `/admin/realms/{realm}/clients/{uuid}/roles` | — | `200` | `POR src/reconciler/app_clients.rs:613`, `:644` | `row_27_to_34_roles_and_role_mappings` |
| 32 | `POST` | `/admin/realms/{realm}/clients/{uuid}/roles` | — | `201` | `POR src/reconciler/app_clients.rs:635` | `row_27_to_34_roles_and_role_mappings`, `k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes` (view-clients: managed App clients only, K3) |
| 33 | `DELETE` | `/admin/realms/{realm}/clients/{uuid}/roles/{role}` | — | `204` | `POR src/reconciler/app_clients.rs:619` | `row_27_to_34_roles_and_role_mappings`, `k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes` (view-clients: managed App clients only, K3) |
| 34 | `GET` | `/admin/realms/{realm}/clients/{uuid}/roles/{role}/groups` and `.../users` | `briefRepresentation`, `max` | `200` | `POR src/reconciler/app_clients.rs:658`, `:670` | `row_27_to_34_roles_and_role_mappings` |
| 35 | `GET` | `/admin/realms/{realm}/clients` | `clientId`, `search`, `briefRepresentation`, `first`, `max` | `200` | `POR src/reconciler/app_clients.rs:413`, `:813`, `workload_clients.rs:509`, `mcp_clients.rs:124`, `charts/admin-permissions/files/sync-admin-permissions.sh:36` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 36 | `POST` | `/admin/realms/{realm}/clients` | — | `201 + Location`, `409` | `POR src/reconciler/app_clients.rs:914`, `workload_clients.rs:733`, `mcp_clients.rs:219` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 37 | `PUT` | `/admin/realms/{realm}/clients/{uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:899`, `workload_clients.rs:716`, `mcp_clients.rs:204`, `mqtt_brokers.rs:1033` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 38 | `DELETE` | `/admin/realms/{realm}/clients/{uuid}` | — | `204` | `POR src/reconciler/app_clients.rs:845`, `workload_clients.rs:568`, `mcp_clients.rs:156`, `mqtt_brokers.rs:978` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 39 | `GET` | `/admin/realms/{realm}/clients/{uuid}/client-secret` | — | `200` | `POR src/reconciler/app_clients.rs:753` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 40 | `POST` | `/admin/realms/{realm}/clients/{uuid}/client-secret` | — | `200` | `POR src/api/account_secret.rs:177` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 41 | `GET` | `/admin/realms/{realm}/clients/{uuid}/service-account-user` | — | `200` | `POR src/reconciler/workload_clients.rs:596`, `src/api/account_secret.rs:205` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 42 | `GET` | `/admin/realms/{realm}/clients/{uuid}/protocol-mappers/models` | — | `200` | `POR src/reconciler/app_clients.rs:463` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 43 | `POST` / `PUT` / `DELETE` | `/admin/realms/{realm}/clients/{uuid}/protocol-mappers/models[/{id}]` | — | `201`, `204` | `POR src/reconciler/app_clients.rs:479`, `:502`, `:520` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 44 | `GET` / `PUT` / `DELETE` | `/admin/realms/{realm}/clients/{uuid}/{default,optional}-client-scopes[/{scope_id}]` | — | `200`, `204` | `POR src/reconciler/app_clients.rs:431`, `workload_clients.rs:622`, `hub_scopes.rs:247`, `charts/admin-permissions/files/sync-admin-permissions.sh:151` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 45 | `GET` / `POST` / `PUT` / `DELETE` | `/admin/realms/{realm}/client-scopes[/{id}]` | — | `200`, `201`, `204`, `409` (name in use) | `POR src/reconciler/hub_scopes.rs:146`, `:162`, `:198`, `:228`, `workload_clients.rs:648`, `charts/admin-permissions/files/sync-admin-permissions.sh:104` | `row_35_to_46_clients_secrets_mappers_and_scopes` |
| 46 | `DELETE` | `/admin/realms/{realm}/attack-detection/brute-force/users/{user_id}` | — | `204` | `components/keycloak/README.md:54` (break-glass) | `row_35_to_46_clients_secrets_mappers_and_scopes` |
