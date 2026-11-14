# Every call the Portal makes to its identity provider

The Portal (`crates/portal`) reaches its realm as Keycloak or as kaisatsu with the same calls. This
page lists each call: method, path, query, the body fields it writes and reads, the status it
expects, where it is made (`crates/portal/src/...:line`) and the test in
`crates/kaisatsu/server/tests/portal_contract.rs` that replays it against kaisatsu's router with
the shipped realm file [`import/realms/jc-portal.json`](import/realms/jc-portal.json).

Paths below are relative to the realm root `{issuer}` = `{root}/realms/{realm}` (OIDC) and to the
admin base `{root}/admin/realms/{realm}` (admin API). `oidc.backchannel()` derives both from
`JC_OIDC_ISSUER`; `JC_KEYCLOAK_ADMIN_URL` overrides the admin root.

## 1. Identities

| Identity | Env | Grant | Roles in the realm file | Writes |
|---|---|---|---|---|
| `portal-api` | `JC_OIDC_CLIENT_ID`, `JC_OIDC_CLIENT_SECRET` (or `JC_OIDC_CLIENT_ASSERTION_FILE`, federated-jwt in the cluster) | code flow, refresh, token exchange, client credentials | `realm-management`: `view-clients`, `query-users`, `query-groups` | App, workload, account, MCP and MQTT clients marked `managed-by: joinedcontext`; their mappers, roles and secrets; Endpoint client scopes marked `managed-by: joinedcontext` without mappers |
| `portal-reconciler` | `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID` / `_SECRET` (`config.keycloak_admin`) | client credentials | `realm-management`: `manage-users`, `query-groups` | people, groups, memberships, role mappings of managed clients |
| `portal-ui` | none (browser) | code flow, `prompt=none` | public client | nothing |

Both admin identities get their token from `POST {issuer}/protocol/openid-connect/token`
(`grant_type=client_credentials`) and send it as `Authorization: Bearer`. Each admin client reads
an error from `errorMessage` or `error`, takes 404 as absent and 409 as a conflict.

## 2. OIDC calls

| # | Call | Sent | Read | Expects | Where | Test |
|---|---|---|---|---|---|---|
| O1 | `GET {issuer}/.well-known/openid-configuration` | | issuer, authorization/token/userinfo/jwks endpoints, `end_session_endpoint` | 200; issuer equal to `JC_OIDC_ISSUER` | auth/oidc.rs:189 | `the_portal_signs_a_person_in_refreshes_and_signs_them_out` |
| O2 | `GET {jwks_uri}` | | RS256 keys (ID tokens, openidconnect), ES256/RS256 keys (bearer tokens, jsonwebtoken 9) | 200 | auth/oidc.rs:189, auth/bearer.rs:101 | same, `Bearer::of` |
| O3 | browser `GET {issuer}/protocol/openid-connect/auth` | `client_id=portal-api`, `redirect_uri={public}/api/v1/auth/callback`, `response_type=code`, `scope=openid …`, `state`, `nonce`, `code_challenge` (S256), `ui_locales` | redirect with `code`, `state` | 302/303 back to the callback | auth/oidc.rs:590 | sign-in test |
| O4 | `POST /token` `authorization_code` | `code`, `redirect_uri`, `code_verifier`, client auth | `access_token`, `id_token` (RS256, header typ JWT, nonce, `sub`, `preferred_username`, `email`, `email_verified`, `name`, `groups` full paths, `realm_access.roles`, `amr`, `jc_org`, `jc_groups`), `refresh_token`, `expires_in` | 200 | auth/oidc.rs:704-800 | sign-in test; `scope=openid gateway-hub` adds `aud: mcp-hub` (COMPAT.md T1) |
| O5 | `POST /token` `refresh_token` | `refresh_token`, client auth | new ID token verified again | 200; `invalid_grant` ends the session | auth/oidc.rs:390 | sign-in test |
| O6 | browser `GET /auth` silent check | `client_id=portal-ui`, `prompt=none`, `response_mode=fragment`, `redirect_uri={public}/api/v1/auth/sso-check/done`, PKCE | fragment `code` or `error=login_required` | 303 | auth/oidc.rs:265 | sign-in test |
| O7 | browser `GET end_session_endpoint` | `id_token_hint`, `client_id`, `post_logout_redirect_uri={public}/` | redirect to the public base | 302/303 | auth/oidc.rs:942 | sign-in test |
| O8 | IdP → Portal `POST {public}/api/v1/auth/backchannel-logout` | `logout_token`: header typ **JWT**, claims `iss`, `aud` portal-api, `sub`, `sid`, `events` back-channel-logout, no `nonce` | | Portal 200 | auth/oidc.rs:1055 (verify :1069) | sign-in test (`OutboundGuard::posted_forms`) |
| O9 | `POST /token` `client_credentials` as portal-api | client auth | `access_token`, `expires_in` | 200 | auth/oidc.rs:336 | `bearer_and_connector_tokens_verify_and_exchange_for_one_endpoint` |
| O10 | `POST /token` token exchange | `subject_token` (person's connector token), subject and requested token types both access tokens, audience `mcp-hub`, scope `gateway-hub endpoint:{slug}` | access token, aud `mcp-hub`, `scope` with `endpoint:{slug}` | 200; refusal `invalid_scope` (scope not linked), `invalid_token` or `invalid_request` "Invalid token" (session ended) | auth/oidc.rs:364, refusals :114/:125 | bearer test |
| O11 | bearer tokens at the Portal API | | ES256/RS256, `iss`, `aud` portal-api (Portal API) or `portal-connectors` (connector), `exp`, `sub`, `typ` Bearer, `azp`; a connector token holds exactly one `connector:{26 chars a-z2-7}` scope | | auth/bearer.rs:203-298, mcp/connector.rs:121, connectors.rs:1472 | bearer test |
| O12 | `POST /token` `client_credentials` as portal-reconciler | client auth | `access_token` | 200 | people.rs:196, reconciler/realm.rs:27, app_clients.rs:493 | every admin test |

The MQTT device page (api/mqtt_devices.rs:177) and the account page (api/account_secret.rs:105)
only print `{issuer}/protocol/openid-connect/token`; the Portal calls no introspection, revocation
or userinfo endpoint.

## 3. Admin calls as portal-reconciler

| # | Call | Sent | Read | Expects | Where | Test |
|---|---|---|---|---|---|---|
| P1 | `GET /users?search&first&max&briefRepresentation=false` | | `KcUser`: id, username, email, firstName, lastName, enabled, emailVerified, createdTimestamp, requiredActions, attributes | 200; no `service-account-*` user; `first`/`max` count people | people.rs:290 | `the_portal_manages_people_as_the_reconciler` |
| P2 | `GET /users/{id}` | | `KcUser`, whole representation for P4 | 200, 404 NotFound | people.rs:304/:309 | people test |
| P3 | `POST /users` | username, email, firstName, lastName, enabled, emailVerified false, requiredActions [VERIFY_EMAIL, UPDATE_PASSWORD], attributes.locale | `Location` last segment | 201; 409 for a taken e-mail | people.rs:327 | people test |
| P3b | `GET /users?email&exact=true` | | first id | 200 | people.rs:341 | people test |
| P4 | `PUT /users/{id}` | the whole representation from P2, edited | | 204; unknown attributes kept | people.rs:361 | people test |
| P5 | `PUT /users/{id}/execute-actions-email?lifespan&client_id&redirect_uri` | actions array | | 2xx sent, 5xx no mail (temporary password fallback); 4xx is an error | people.rs:383 | people test |
| P6 | `PUT /users/{id}/reset-password` | type password, value, temporary true | | 204 | people.rs:408 | people test |
| P7 | `POST /users/{id}/logout` | | | 204 | people.rs:418 | people test, bearer test |
| P8 | `GET /users/{id}/credentials`, `DELETE /users/{id}/credentials/{cid}` | | id, type (otp, webauthn, webauthn-passwordless removed) | 200 / 204 | people.rs:426/:434 | people test (GET) |
| P9 | `GET /users/{id}/sessions` | | lastAccess | 200 | people.rs:445 | people test |
| P10 | `GET /users/{id}/consents`, `DELETE /users/{id}/consents/{client}` | | clientId, grantedClientScopes, createdDate, lastUpdatedDate | 200 / 204 | people.rs:452/:461 | people test (GET) |
| P11 | `GET /users/{id}/groups`, `GET /users/{id}/role-mappings/realm/composite` | | path; name (`offline_access` from the realm's default roles) | 200 | people.rs:483-497 | people test |
| P12 | `GET /groups?search=members&exact=true&briefRepresentation=true&max=1000`, then `PUT /users/{id}/groups/{gid}` | | tree: id, path, subGroups; finds `/{org}/members` | 200 / 204 | people.rs:507/:513 | people test |
| P13 | `DELETE /users/{id}` | | | 204 | people.rs:519 | people test |
| G1 | `GET /groups?briefRepresentation=false&max=1000` | | id, name, attributes (managed-by, jc_org, jc_groups) | 200 | reconciler/groups.rs:207 | `the_group_wave_converges_as_the_reconciler` |
| G2 | `POST /groups`, `GET /groups?search={name}&exact=true` | name, attributes | | 201 / 200 | groups.rs:331/:344 | group test |
| G3 | `GET /groups/{id}/members?max=1000`, `PUT`/`DELETE /users/{u}/groups/{g}`, `GET /users/{u}` | | member ids; 404 absent | 200 / 204 | groups.rs:385/:403/:428/:486 | group test |
| G4 | `PUT /groups/{id}`, `DELETE /groups/{id}` | name, attributes | | 204 | groups.rs:471/:288 | group test |
| A1 | `GET /groups?briefRepresentation=true&max=1000`, `GET /users/{id}` or `/users?exact=true&{email\|username}=` | | name→id; id, username, email | 200, 404 absent | app_clients.rs:799/:825/:841 | `the_app_wave_converges_an_app_client` |
| A2 | `POST`/`DELETE /{groups\|users}/{id}/role-mappings/clients/{uuid}` | `[{id, name}]` of an App client role | | 204 | app_clients.rs:929-952 | App test |

## 4. Admin calls as portal-api

| # | Call | Sent | Read | Expects | Where | Test |
|---|---|---|---|---|---|---|
| C1 | `GET /clients?clientId={id}` | | first client, `id`, attributes (managed-by, owner attribute) | 200 | app_clients.rs:601 | every client test |
| C2 | `POST /clients` (App) | app_clients.rs:163 `desired`: no `defaultClientScopes` | | 201; the realm's default scopes linked (roles, profile, email, basic, web-origins) | app_clients.rs:1196 | App test |
| C3 | `PUT /clients/{uuid}` | the desired body | | 204; the next GET shows no drift (`drift`, array fields as sets) | app_clients.rs:1181 | App test |
| C4 | `GET`/`DELETE /clients/{uuid}/optional-client-scopes[/{sid}]` | | id, name | 200 / 204 | app_clients.rs:620 | App, MCP test |
| C5 | `GET`/`POST`/`PUT`/`DELETE /clients/{uuid}/protocol-mappers/models[/{mid}]` | audience-, groups, jc_org, jc_groups mappers | id, name, protocolMapper, config | 200 / 201 / 204 | app_clients.rs:655-735 | App, workload, MCP test |
| C6 | `GET`/`POST /clients/{uuid}/roles`, `DELETE /clients/{uuid}/roles/{r}` | name | id, name | 200 / 201 / 204 | app_clients.rs:870-900 | App test |
| C7 | `GET /clients/{uuid}/roles/{r}/groups?briefRepresentation=true&max=1000`, `…/users?max=1000` | | group id, name; user id, username, email | 200 | app_clients.rs:908-918 | App test |
| C8 | `GET /clients/{uuid}/client-secret` | | value | 200 | app_clients.rs:1003 | App test |
| C9 | `GET /clients?search=true&clientId=app-&max=1000`, `DELETE /clients/{uuid}` | | clientId, attributes | 200 / 204 | app_clients.rs:1067/:1107 | App test |
| W1 | `GET /clients?briefRepresentation=false&max=2000` | | every client | 200 | workload_clients.rs:744 | `the_client_waves_converge_as_portal_api` |
| W2 | `POST /clients` (workload, external account) | `desired` / `desired_external` with `defaultClientScopes [basic]`, `optionalClientScopes []` | | 201; no drift | workload_clients.rs:60/:98/:161/:985 | client waves test |
| W3 | `GET /clients/{uuid}/{default,optional}-client-scopes`, `DELETE …/{sid}`, `GET /client-scopes`, `PUT /clients/{uuid}/default-client-scopes/{sid}` | | `basic` listed by `GET /client-scopes` | 200 / 204 | workload_clients.rs:867-908 | client waves test |
| W4 | `GET /clients/{uuid}/service-account-user`, `DELETE /clients/{uuid}` | | id, username | 200 / 204 | workload_clients.rs:839/:809 | client waves test |
| S1 | `POST /clients/{uuid}/client-secret` | | value; the old secret stops working | 200 | api/account_secret.rs:192 | client waves test |
| M1 | MCP client `POST`/`PUT`/`DELETE /clients…`, mappers, optional scopes | mcp_clients.rs:42 `desired` (public, consentRequired, PKCE) | | 201 / 204 | mcp_clients.rs:127-285 | client waves test |
| Q1 | MQTT broker client `POST`/`PUT`/`DELETE /clients…` | mqtt_brokers.rs:897 (bearer-only) | | 201 / 204 | mqtt_brokers.rs:950-1033 | client waves test |
| H1 | `GET /client-scopes`, `POST /client-scopes`, `PUT`/`DELETE /client-scopes/{id}` | hub_scopes.rs:61 `desired`: name `endpoint:{slug}`, attributes include.in.token.scope, display.on.consent.screen, consent.screen.text, managed-by, joinedcontext.endpoint; no mappers | | 200 / 201 / 204 | hub_scopes.rs:154-241 | bearer test, client waves test |
| H2 | `GET /clients/{mcp-hub}/{default,optional}-client-scopes` | | names | 200 | hub_scopes.rs:271 | client waves test |

## 5. What kaisatsu refuses although Keycloak might allow it

The Portal needs none of these; each refusal has its test in `portal_contract.rs` or
`admin/tests/authz_regressions.rs`.

- portal-api writes no client scope without `managed-by: joinedcontext`, none with a protocol
  mapper, and none under a name the realm already holds (409). Keycloak's FGAP `manage` grant on
  the client type would let it rewrite `roles` or `connector`.
- portal-api links no scope to `mcp-hub`, to itself or to any unmanaged client: the deployment's
  admin-permissions Job (a realm admin) links `endpoint:` scopes. In the one binary that Job does
  not exist: a realm-file client with the attribute `kaisatsu.linked-scope-prefix: endpoint:`
  (`portal-api`, `mcp-hub` in `jc-portal.json`) takes each `endpoint:{slug}` scope the Portal
  marks (`managed-by: joinedcontext`, `joinedcontext.endpoint`, no mapper, an opaque slug) as
  optional, resolved at each token request and listing, so a deleted scope is gone at once and
  `endpoint:*` or an unmarked scope is never taken (T-6857).
- portal-api writes no unmanaged client (`portal-reconciler`, `mcp-hub`) and creates no person.
- portal-reconciler lists, reads and writes no client.
- `POST /users` stores no role mapping, group or service-account link from the body, for every
  caller below realm admin; the person holds the realm's `defaultRoles`.
- A new client gets the realm's default scopes only when the body names none, and never the
  realm's default optional scopes (`connector`, `offline_access`): Keycloak links those, and the
  Portal removes them again (T-6301).
- `manage-users` maps the roles of managed clients only (Keycloak: any non-admin role).
