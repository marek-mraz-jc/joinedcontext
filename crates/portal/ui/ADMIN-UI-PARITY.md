# Administering the platform in the Portal UI: parity with the MCP operations

Owner order 2026-11-08: every administration action a platform administrator (P), an organization
administrator (O) or a project administrator (A) can take through the MCP door can also be taken
in the Portal UI, with the same rights. The rights are not the UI's: each screen reads
`GET /api/v1/projects/{project}/permissions/me` only to say early what the person may not do, and
the route behind the control decides again on every call (PF-51). The persona matrix of the
operations is `crates/portal/MCP-ADMIN.md` on the `jc-mcp-admin` branch; this file maps each
operation that writes or administers to the page and control that do the same.

Status words: **covered** (the control calls the same route, or the route the operation calls),
**equivalent** (the UI reaches the same result through another route, named), **new** (built on
the `jc-admin-ui` branch), **UI only** (the MCP refuses it on purpose; the Portal session is the
only door), **n/a** (no person-facing action).

## 1. Operations of the registry that write or administer

| Operation | Route it calls | UI page (address) | Control (file) | Status |
|---|---|---|---|---|
| `jc_person_create` | `POST /organization/people` | `/organization/people` | "Create person", `src/pages/organization/People.tsx` | covered |
| `jc_person_edit` | `PATCH /organization/people/{id}` | `/organization/people/{id}` | "Edit", "Save", `People.tsx` | covered |
| `jc_person_disable` / `_enable` | `POST …/people/{id}/disable`, `/enable` | `/organization/people/{id}` | "Disable" (also bulk), "Enable", `People.tsx` | covered |
| `jc_person_sign_out` | `POST …/people/{id}/sign-out` | `/organization/people/{id}` | "Sign out everywhere", `People.tsx` | covered |
| `jc_person_list` / `_get` | `GET /organization/people[/{id}]` | `/organization/people` | the people table and person page, `People.tsx` | covered |
| `jc_project_create` | `POST /projects` | sidebar, `/organization/projects` | "New project", `src/components/layout/NewProject.tsx` (disabled with the server's reason when the Organization does not let the person open one) | covered |
| `jc_project_delete` | `DELETE /projects/{p}` | `/projects/{p}/settings/danger`, `/organization/projects` | "Delete", `src/components/DeleteProjectDialog.tsx` | covered |
| `jc_project_export` / `_revisions` | `GET /projects/{p}/export`, `/revisions` | `/organization/projects` | "Export", revision picker, `src/components/export/ExportModal.tsx` | covered |
| `jc_project_import` | `POST /projects/{p}/import` | `/organization/projects`, `/projects/{p}/import` | "Import project", "Propose the import", `ImportProjectDialog.tsx`, `src/pages/import/ImportPage.tsx` | covered |
| `jc_project_get` | `GET /projects/{p}` | `/projects/{p}/settings/general` | `src/pages/projectSettings/ProjectSettingsPage.tsx` | covered |
| `jc_resource_propose` | `POST /projects/{p}/{plural}`, `PUT …/{name}` | `/projects/{p}/{plural}/new`, `…/{name}/edit`; at `org`: `/organization/{tab}` | "Check", "Propose change", `src/components/ResourceFormDialogBody.tsx` | covered |
| `jc_resource_delete` | `DELETE /projects/{p}/{plural}/{name}` | the row menu of every list | "Propose removal", `src/components/DeleteResourceDialog.tsx` | covered |
| `jc_resource_get` / `_list`, `jc_schema_get` | `GET /projects/{p}/{plural}[/{name}]` | every list and detail page | `src/routes/ResourceListPage.tsx`, `src/components/grid/ManifestGrid.tsx` | covered |
| `jc_endpoint_propose` | `POST /projects/{p}/endpoints` | `/projects/{p}/endpoints/new` | "Propose change", `src/routes/EndpointsPage.tsx` | covered |
| `jc_endpoint_list_all` | `GET /endpoints` (all projects) | `/organization/endpoints` | `src/routes/AllEndpointsPage.tsx` | covered |
| `jc_endpoint_preview` | `POST /endpoints/{name}/preview` | `/projects/{p}/endpoints/{name}` | `FilterProof.tsx` (runs on edit) | covered |
| `jc_access_simulate` | `POST /endpoints/{name}/access/simulate` | `/projects/{p}/policies` | "Ask the gateway", `TryAPerson.tsx` | covered |
| `jc_access_explore` | `POST /access/explore` | `/projects/{p}/settings/explorer` | "Show access", `src/pages/access/AccessExplorer.tsx` | covered |
| `jc_attribute_explain` | `GET /endpoints/{name}/access/attributes` | `/projects/{p}/endpoints/{name}` | `src/api/access.ts` | covered |
| `jc_member_list` | the project's RoleBindings | `/projects/{p}/settings/members` | "Grant a role", `src/pages/access/RoleBindings.tsx` | covered |
| `jc_space_complete` | operation (`POST /projects/{p}/ops/jc_space_complete`) | `/projects/{p}/spaces/complete`, `/spaces/upload` | "Propose all", `SpaceComplete.tsx`; `UploadCsv.tsx` | covered |
| `jc_space_governance` | quality, checks, usage, temporal audit, surface fit, drift | `/projects/{p}/spaces/{s}` | `SpaceQuality.tsx`, `SpaceChecks.tsx`, `SpaceTemporalAudit.tsx`, `SpaceDrift.tsx` | covered |
| `jc_syncsource_sync` / `_pause` / `_detach` / `_status` | `POST /projects/{p}/syncsources/{n}/sync`, `/pause`, `/detach` | `/projects/{p}/syncsources` | "Sync now", "Pause"/"Resume", "Detach", `src/pages/sync/SyncSourcesPage.tsx` | covered |
| `jc_model_source_get` / `_put` | `GET`/`PUT /datamodels/{n}/source` | `/projects/{p}/models?edit=…` | "Check", "Propose v{n}", `ModelsPage.tsx` | covered |
| `jc_model_sdm_search` / `_import` | `POST /tools/import-sdm` | `/projects/{p}/models?new=sdm` | `SmartDataModelsImport.tsx` | covered |
| `jc_model_generate`, `jc_model_infer` | `POST /tools/generate`; operation | models editor, `/projects/{p}/models?new=file` | `LinkmlPreviewPanel.tsx`; "Read the address", `ModelFileDrop.tsx` | covered |
| `jc_datasource_check`, `jc_manifest_dry_run` | `POST /datasources/{n}/test`, `?dryRun=All` | `/projects/{p}/datasources`, every form | `ConnectionTest.tsx`, "Check" | equivalent |
| `jc_pipeline_run` | `POST /pipelines/{n}/runs` | `/projects/{p}/pipelines/{n}` | "Run now", `PipelineRuns.tsx` | covered |
| `jc_pipeline_test` / `_validate` / `_sample_source` / `_try_mapping` | `POST /pipelines/test`; operations | `/projects/{p}/pipelines/new`, editor | `PipelineTest.tsx`, `PipelineWorkbench.tsx` | covered |
| `jc_pipeline_metrics` / `_inspect` | `GET /pipelines/{n}/metrics`, runs, logs | `/projects/{p}/pipelines/{n}` | `PipelineRuns.tsx`, `PipelineOverview.tsx` | covered |
| `jc_flow_start` | `POST /projects/{p}/flows` | `/projects/{p}/datasources` | "Set up", `src/pages/flows/Instantiate.tsx` | covered |
| `jc_mqtt_test_publish` | `POST /mqttbrokers/{n}/test-publish` | `/projects/{p}/mqttbrokers/{n}` | "Run the test", `TestPublish.tsx` | covered |
| `jc_catalogue_draft`, `jc_ckan_status` | `POST /catalogue/drafts`; CKAN status | `/catalogue`, `/projects/{p}/ckan` | "Draft the description", `PublishDataset.tsx` | covered |
| `jc_change_list` / `_get` / `_history` | `GET /changes`, `/changes/{id}`, `/changes/history` | `/projects/{p}/approvals`, `/projects/{p}/inbox` | `ApprovalsPage.tsx`, `ApprovalsHistory.tsx`, `InboxPage.tsx` | covered |
| `jc_change_revert_plan` | operation | `/projects/{p}/approvals` (history) | "Restore", "Undo" (`POST /changes/{id}/restore`, `/undo`), `ApprovalsHistory.tsx` | equivalent |
| `jc_change_plan` / `jc_change_apply` | operations (a connector's multi-object plan) | every form | a form proposes its object directly after a green "Check" | equivalent |
| `jc_draft_list` / `_get` / `_put` | `GET`/`PUT /drafts/{kind}/{name}` | every resource form | autosave, `src/components/forms/useResourceDraft.ts`; home page draft count | covered |
| `jc_draft_drop` | `DELETE /drafts/{kind}/{name}` | every resource form holding a saved draft | "Discard the draft" with a second confirmation, `src/components/forms/ResourceFormFooter.tsx` (`DiscardDraft`) | **new** |
| `jc_activity_list` | `GET /projects/{p}/activity` | `/projects/{p}/activity` | `src/routes/ActivityPage.tsx` | covered |
| `jc_workspace_*` | `/projects/{p}/workspaces…` | `/projects/{p}/workspaces…` | `WorkspacesPage.tsx`, `BringBackPage.tsx`, `TryItPage.tsx`, `ComparePage.tsx` | covered |
| `jc_run_*` | `/agent-runs…` | app generator, `/organization/agents` | `AppGenerator.tsx`, "Stop" in `AgentActivity.tsx` | covered |
| `jc_data_write` | gateway `/api/endpoint/{slug}/…` with an exchanged token | `/projects/{p}/spaces/{s}` | the row form and bulk set write through `/cs/{space}/ngsi-ld/v1/entities` | equivalent |
| `jc_kpi_compute` | operation | `/projects/{p}/pipelines/new` (KPI recipe) | `KpiEditor.tsx` checks through `/kpis/check` | equivalent |

## 2. The new administration operations of `jc-mcp-admin`

| Operation | Route it calls | UI page (address) | Control (file) | Status |
|---|---|---|---|---|
| `jc_organization_read` aspect `projects` | `projects::list_projects` | `/organization/projects`; the project switcher | `OrganizationPage.tsx` (`OrganizationProjects`) | covered |
| aspect `setup` | `setup::get_setup` | `/organization/setup` | `src/pages/organization/OrganizationSetup.tsx` | covered |
| aspect `health`, `healthChecks` | `validation::get_health`, `health_checks::list_all` | `/organization/health` | `ValidationHealth.tsx`, `HealthLog.tsx` | covered |
| aspect `limits` | `organization_limits::get_limits` | `/organization/settings` | `OrganizationSettings.tsx` | covered |
| aspect `brokers` | `brokers::list_brokers` | `/organization/brokers` | `src/pages/organization/Brokers.tsx` | covered |
| aspect `agents`, `agentActivity` | `agent_registry::list_agents`, `agent_activity::list_agent_activity` | `/organization/agents` | `Agents.tsx`, `AgentActivity.tsx` | covered |
| `jc_access_request_list` | `GET /projects/{p}/access-requests` | `/projects/{p}/inbox` (every space of the project) and `/projects/{p}/endpoints/{n}` (that space) | "Access requests" section, `InboxPage.tsx` with `AccessRequestInbox` | **new** (project-wide list) |
| `jc_access_request_decide` | `POST …/access-requests/{id}/approve`, `/revoke` | same | "Approve", "Revoke" with a reason (hint with an example), `src/pages/pipelines/AccessRequests.tsx` | **new** (from the Inbox) |
| `jc_serviceaccount_access` | `agent_access::account_access` | `/projects/{p}/settings/service-accounts`, `/organization/service-accounts` | `src/pages/access/AccountAccess.tsx` | covered |

## 3. Actions the MCP refuses on purpose and the UI keeps

These answer "refused" in the MCP matrix because a person has to be present (AG-11, AG-133,
PF-58); the Portal session is their door, so they need no operation.

| Action | UI page | Control |
|---|---|---|
| Approve or reject a Change | `/projects/{p}/approvals/{id}`, `/projects/{p}/inbox` | "Approve", "Reject", `ApprovalDetailPage.tsx`, `InboxPage.tsx` |
| Decline an access request | `/projects/{p}/inbox`, endpoint page | "Decline" with a reason, `AccessRequests.tsx` |
| Delete a person, reset a password, resend an invitation, remove a second factor | `/organization/people/{id}` | `People.tsx` |
| Issue an account's client secret, revoke an account's grant | service accounts tabs | `AccountSecret.tsx`, "Revoke now" in `AccountAccess.tsx` |
| Every person's connectors, revoke or approve one | `/organization/connectors` | `OrganizationConnectors.tsx` |

## 4. What each persona sees (e2e)

`e2e/admin_personas.spec.ts` walks each persona at 1440 and 400 px with route stubs that answer
as the server answers that person:

- P: Administration settings, "New project" (POST `/projects`), then space, endpoint and
  pipeline in `helsinki`, each a checked proposal.
- O: its own Organization's settings, space, endpoint and pipeline in `ovzdusie`; refused an
  installation section (`/organization/brokers`, "Ask the installation's administrators", with
  the installation's contacts and a link to its own sections), "New project" disabled with the server's reason, and a project of another
  Organization answers 404 under the page's own header; no write reaches that project.
- A: space, endpoint and pipeline in `helsinki`, an access request decided from the Inbox;
  refused `/organization/settings` and another project; no write reaches `org`.

## 5. Gaps left

- Opening a project inside a non-installation Organization, people per Organization, Grafana
  visualizations, space move and retention preview: gaps of the operations side, listed in
  `MCP-ADMIN.md` section 5; the UI has no extra control for them either.
- Purging a broker tenant (`DELETE /q/tenants/{t}`) stays a deployer runbook step, in neither door.
