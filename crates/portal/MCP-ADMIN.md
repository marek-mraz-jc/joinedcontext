# Administering the platform through MCP: persona matrix

Owner order 2026-11-08: a platform administrator, an organization administrator and a project
administrator each set up and run the platform through the Portal's MCP door
(`/api/v1/mcp`), and each sees and does only what their role allows. Every row below is an
operation of the registry (`src/ops`), so the same name answers at `POST
/api/v1/projects/{project}/ops/{name}`, in `tools/list` and to the UI. There is no second
permission model: the personas are RoleBindings of the existing `Role`s.

## 1. Personas

| Persona | What makes it | Where its grants reach |
|---|---|---|
| Platform administrator (P) | `org-admin` bound with `scope: {organization: <installation Organization>}` (or the bootstrap group `JC_PORTAL_BOOTSTRAP_ADMINS`) | Every project of the installation Organization and the organization namespace `org` (bindings, groups, brokers, every `Organization`). `Effective::administers_organization()` holds. A project of another Organization is that Organization's own: P reads it as not there (404, ADR-N-058 §2.4); only the bootstrap group reaches everything, for the first start. |
| Organization administrator (O) | `org-admin` bound with `scope: {organization: <another Organization>}` (ADR-N-058 §2.4, ADR-N-067) | The projects whose `Project.spec.organizationRef` names that Organization, and its own `Organization` settings, bindings and groups at `org` (narrowed). Never the installation-wide manifests, never another Organization. |
| Project administrator (A) | `steward` (read, propose, approve, delete on the project's kinds) bound with `scope: {project: <p>}` | That one project. A project it is not bound in answers 404 (PF-59, R20). |

Legend: **op** = exists, the persona may run it; **op (route)** = exists, the route decides per
call and refuses this persona; **refused** = refused for this persona on purpose (rule named);
**gap** = no operation yet (the exact call is in section 4). **UI page** is the Portal route
that shows the action today (`ui/src/router.tsx`); "none yet" is a screen jc-admin-ui builds on
the operation named in the row. Every operation is on the registry, so the UI reads it through
`POST /api/v1/projects/{project}/ops/{name}` and `GET .../permissions/me`.

## 2. Matrix

| Area | Action | P | O | A | UI page |
|---|---|---|---|---|---|
| Organizations | read / edit settings | `jc_resource_get`, `jc_resource_propose` kind `Organization` | same, own Organization only (narrowed) | refused (PF-03) | `/organization/settings` |
| | create another Organization | `jc_resource_propose` kind `Organization` at `org` | refused: installation-wide manifest (ADR-N-058 §2.2) | refused | none yet (jc-admin-ui) |
| | setup checklist, validation health, every project's checks, limits, brokers | `jc_organization_read` aspects `setup`, `health`, `healthChecks`, `limits`, `brokers` (new) | refused by the route (PF-03: installation administrators) | refused by the route | `/organization/setup`, `/organization/health`, `/organization/limits` (OrganizationLimitsView), `/organization/brokers`; project checks: `/projects/$project/health` |
| Projects | list readable projects | `jc_organization_read` aspect `projects` (new) | same; only its Organization's projects | same; only its projects | `/` (project picker), `/organization/projects` |
| | open a project | `jc_project_create` | refused: a new project belongs to the installation Organization (ADR-N-058 §2.2); P moves it by `organizationRef` | refused unless `Organization.spec.projects.creation` lets them (PF-65) | `/` New project |
| | read, revisions, export, import, delete | `jc_project_get`, `jc_project_revisions`, `jc_project_export`, `jc_project_import`, `jc_project_delete` (red lane, asks) | same, in its Organization's projects | same, in its project; export of the whole project is P's | `/projects/$project/settings`, `/projects/$project/import` |
| Spaces | create, edit, complete, delete | `jc_resource_propose` / `jc_resource_delete` kind `ContextSpace`, `jc_space_complete`, `jc_space_governance` | same in its projects | same in its project | `/projects/$project/spaces`, `/projects/$project/spaces/complete` |
| People (kaisatsu realm) | list, get, create, edit, disable, enable, sign out | `jc_person_list`, `jc_person_get`, `jc_person_create`, `jc_person_edit`, `jc_person_disable`, `jc_person_enable`, `jc_person_sign_out` | op (route): the realm is the installation's; the route decides on `Person` at organization scope | refused by the route | `/organization/people` |
| | delete, reset password, resend invitation, remove second factor, revoke consent | refused on purpose: a person in a Portal session only (AG-133, PF-58, `refuse_bearer`) | refused | refused | `/organization/people` (session only) |
| Groups, Roles, RoleBindings | create, edit, delete | `jc_resource_propose` / `jc_resource_delete` kinds `Group`, `Role`, `RoleBinding` | same, its own Organization's (narrowed) | project `RoleBinding`s and project `Role`s; never a right the caller lacks (PF-93) | `/organization/groups`, `/organization/roles`, `/projects/$project/settings/members` |
| | project members | `jc_member_list` | same | same | `/projects/$project/settings/members` |
| Connectors and delegation | DelegationPolicy | `jc_resource_propose` kind `DelegationPolicy` | same, own Organization | refused (org kind) | none yet (jc-admin-ui) |
| | every person's connectors, revoke one, approve a pending one | refused on purpose: an administrator in a Portal session only (AG-133, `org_administrator`) | refused | refused | `/organization/connectors` (session only) |
| Data models | read, source, put, SDM search/import, generate, infer | `jc_resource_*` kind `DataModel`, `jc_model_source_get`, `jc_model_source_put`, `jc_model_sdm_search`, `jc_model_sdm_import`, `jc_model_generate`, `jc_model_infer` | same in its projects | same in its project | `/projects/$project/models`, `/organization/models` |
| Endpoints and policies | propose, preview, simulate, explore, explain | `jc_endpoint_propose`, `jc_resource_propose` kinds `Endpoint`, `Policy`, `jc_endpoint_preview`, `jc_access_simulate`, `jc_access_explore`, `jc_attribute_explain`, `jc_endpoint_list_all` | same in its projects | same in its project | `/projects/$project/endpoints`, `/projects/$project/policies`, `/projects/$project/access` |
| | access requests: list, approve (proposes the grant), revoke (proposes taking it back) | `jc_access_request_list`, `jc_access_request_decide` (new) | same in its projects | same in its project; never one it filed (PL-91) | requester side `/projects/$project/pipelines/$name` (AccessRequests); owner side: none yet (jc-admin-ui, reads `jc_access_request_list`) |
| | decline an access request | refused on purpose: a person in the Portal (AG-133) | refused | refused | owner-side page, session only (jc-admin-ui) |
| Pipelines (ekiben runner) | propose, validate, test, sample, try mapping, run, metrics, inspect, templates | `jc_resource_propose` kind `Pipeline`, `jc_pipeline_validate`, `jc_pipeline_test`, `jc_pipeline_sample_source`, `jc_pipeline_try_mapping`, `jc_pipeline_run`, `jc_pipeline_metrics`, `jc_pipeline_inspect` | same in its projects | same in its project | `/projects/$project/pipelines` |
| Forge repos and branch rules (shako) | project repository, revisions, changes | `jc_project_get`, `jc_project_revisions`, `jc_change_list`, `jc_change_get`, `jc_change_history`, `jc_change_plan`, `jc_change_apply` (connectors) | same in its projects | same in its project | `/projects/$project/approvals`, `/projects/$project/settings` |
| | approve or reject a Change | refused on purpose over MCP: a person in the Portal (AG-11, AG-133) | refused | refused | `/projects/$project/approvals/$id` (session only) |
| | branch protection | refused on purpose: the Portal sets it when it opens a repository (`gitea.rs` `protect_branch`); no door edits it, or review could be bypassed | refused | refused | none (set by the Portal) |
| Catalogue publishing (dataroom) | CKAN instance, publish an Endpoint, status, drafts | `jc_resource_propose` kinds `CkanInstance`, `Endpoint` (`publish.ckan`), `jc_ckan_status`, `jc_catalogue_draft`, `jc_catalog_search` | same in its projects | same in its project | `/projects/$project/ckan`, `/catalogue` |
| Dashboards (daiya) | Dashboard manifests | `jc_resource_propose` / `jc_resource_delete` kind `Dashboard` | same in its projects | same in its project | `/projects/$project/dashboards`, `/projects/$project/grafana` |
| Broker tenants (Antares) | brokers and sizes | `jc_organization_read` aspect `brokers` (new); `jc_resource_propose` kind `ContextBroker` at `org` | refused (installation-wide) | refused | `/organization/brokers` |
| | a space's tenant | follows its `ContextSpace` (reconciler) | same | same | `/projects/$project/spaces` |
| | purge a tenant's rows (`DELETE /q/tenants/{t}`) | gap, on purpose: irreversible data loss; deployer runbook only | refused | refused | none |
| API keys / ServiceAccounts | create, edit, delete | `jc_resource_propose` / `jc_resource_delete` kind `ServiceAccount` | same in its projects | same in its project | `/organization/groups`, `/organization/roles`, `/projects/$project/settings/members` |
| | what an account may reach | `jc_serviceaccount_access` (new) | same in its projects | same in its project | `/projects/$project/serviceaccounts/$name` (AccountAccess) |
| | issue a client secret | refused on purpose: a secret is never an answer to a tool (secret_scan, AG-133); the Portal page shows it once | refused | refused | `/projects/$project/serviceaccounts/$name` (AccountSecret, session only) |
| | revoke an account's grant | refused on purpose: `revoke_grant` refuses a client token (PF-58, AG-11) | refused | refused | `/projects/$project/serviceaccounts/$name` (session only) |
| Audit / activity | project activity | `jc_activity_list` | same in its projects | same in its project | `/projects/$project/activity` |
| | what agent runs did, agent profiles | `jc_organization_read` aspects `agentActivity`, `agents` (new) | same, runs of its projects | same, runs of its project | `/organization/agents` (Agents, AgentActivity) |
| Agent runs | any administration operation | refused: `administration::refuse_agent` (AG-11, AG-70) | refused | refused | n/a |

## 3. What the new operations do

All four live in `src/ops/administration.rs` and call the route's own function, so the check is
the route's. Each has a closed input schema (validated at the door, `within_schema`, then the
operation's own `validate`), an `outputSchema` (so MCP answers carry `structuredContent`), a
declared kind and verb rule (`requires`), the secret scan of `ops::call` on every argument and
the masked answer for MCP, and one activity row per MCP `tools/call` (`mcp::record_call`).

| Operation | Kind / rule | Lane | Route it calls |
|---|---|---|---|
| `jc_organization_read` | `*`, read in the project; each aspect's route decides | green | `projects::list_projects`, `setup::get_setup`, `validation::get_health`, `health_checks::list_all`, `organization_limits::get_limits`, `brokers::list_brokers`, `agent_registry::list_agents`, `agent_activity::list_agent_activity` |
| `jc_access_request_list` | `ContextSpace`, read in the project | green | `access_requests::list_incoming` |
| `jc_access_request_decide` | approve on `ContextSpace`, checked by the route (offered only to who approves some space; the route checks the request's own space and that the caller did not file it) | yellow (asks) | `access_requests::approve_request`, `access_requests::revoke_request` |
| `jc_serviceaccount_access` | `ServiceAccount`, read in the project | green | `agent_access::account_access` |

## 4. Fixed on the way

- A proposer's check was recorded only when `check(kind, propose, None)` held
  (`ops/proposals.rs` `record_verdict`). A grant narrowed to one space or to an Organization's own
  settings admits nothing without a target, so an organization administrator's dry run of their
  own `Organization` was dropped and the proposal answered `409 verdict_required` forever. The
  check now reads the manifest, as the proposal does. Red proof:
  `an_organization_administrator_runs_its_own_organization_and_no_other` fails with the old line.

## 5. Gaps left

- Opening a project directly inside a non-installation Organization (O): `POST
  /api/v1/projects` always writes `organizationRef: <installation>` (`api/projects.rs`
  `open_project_files`). Today P opens it and proposes the `Project` manifest with the other
  `organizationRef` (`jc_resource_propose`). Changing that is an ADR-N-058 §2.2 decision.
- People of a non-installation Organization (O): the realm is one per installation, so
  `jc_person_*` are P's. A per-Organization people view needs kaisatsu organizations first.
- Grafana visualizations (`POST /projects/{p}/grafana`, `DELETE /projects/{p}/grafana/{id}`)
  have no operation; Dashboard manifests cover daiya through `jc_resource_propose`.
- Space move (`POST /projects/{p}/spaces/{s}/move`) and retention preview have no operation.
- Sibling URLs: until `jc-portal-integrate` wires kaisatsu, shako, ekiben, dataroom and daiya
  into the Portal settings, these operations reach whatever Keycloak, forge, runner, CKAN and
  Grafana the Portal's config names; the tests run against the Portal's mocks.

## Related

- `src/ops/administration.rs`, `src/ops/mod.rs` (`Caller::may_run`)
- `tests/mcp_admin_personas_tests.rs`
- ADR-N-058 (organizations), ADR-N-067 (organization grants), AG-11, AG-133, PF-03, PF-58, PF-59
