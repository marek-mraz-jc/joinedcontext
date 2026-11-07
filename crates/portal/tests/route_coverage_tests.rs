//! Every route the Portal serves is an operation of the registry or says why it is not (AG-59,
//! CC-48, T-0840). Moved out of `src/ops/mod.rs` (T-1499): the table is data about the whole
//! crate, and the registry's module keeps the registry.

use joinedcontext_portal::ops;

#[path = "common/routes.rs"]
mod routes;

use routes::{routes_in, routes_in_source};

/// Every route the Portal serves, and the operation behind it (AG-59, CC-48, T-0840).
///
/// The third column is the operation's name, or the sentence saying why the route has none.
/// A route with neither fails the test below, so a new door is a decision rather than
/// something only a browser ever knew about.
const ROUTE_COVERAGE: &[(&str, &str, &str)] = &[
("GET", "/", "the front door: a signed-in person gets the Portal shell, anyone else a redirect to the public catalogue; no data (T-5627, EP-81)"),
("GET", "/.well-known/oauth-protected-resource", "the OAuth resource metadata an MCP client reads before it authenticates"),
("GET", "/.well-known/oauth-protected-resource/api/v1/mcp", "the OAuth resource metadata an MCP client reads before it authenticates"),
("POST", "/activity", "the runner's OTLP ingest: a workload writes what happened, nobody calls it"),
("GET", "/apps/{name}/", "a published application, served as files"),
("GET", "/apps/{name}/{*path}", "a published application, served as files"),
("GET", "/edge/apps/{name}/admit", "the edge's forward-auth before a pod or wasm App answers: the static host's own visibility check, nothing an agent calls (T-4922)"),
("GET", "/docs/{chapter}/{page}", "the API and User Guide pages as HTML for a signed-in person, no data (T-4063, T-6200, API/00 §1)"),
("GET", "/assistant-preview/{public_id}", "the embed dialog's launcher preview on a blank page: a public id's launcher line under its own CSP, no data (T-5328)"),
("GET", "/templates/{name}", "an App template's demo on its fixtures, served as files, no data (AP-141)"),
("GET", "/templates/{name}/", "an App template's demo on its fixtures, served as files, no data (AP-141)"),
("GET", "/templates/assets/{file}", "the App templates' demo bundle, served as files (AP-141)"),
("POST", "/apps/{name}/api/functions/{fn}", "a published application's own function, called by its page with the person's edge token (AP-84, SDK-23); what it does is the application's, not an operation of the platform"),
("POST", "/apps/{name}/api/services/ai/complete", "a published App's page asks the platform's model with the person's edge token, within the App's aiTokensPerDay (AP-169, API/06 §3); an agent has its own model calls through its run"),
("GET", "/apps/{name}/api/services/jobs", "a published App's page lists its own scheduled jobs with the person's edge token (AP-162, API/06 §3); an agent reads the App's status.jobs with jc_resource_get"),
("GET", "/apps/{name}/api/services/files", "a published App's page keeps its own objects under its prefix with the person's edge token (AP-170, API/06 §3); an agent changes an App by a run, never its files"),
("GET", "/apps/{name}/api/services/files/{*path}", "a published App's page keeps its own objects under its prefix with the person's edge token (AP-170, API/06 §3); an agent changes an App by a run, never its files"),
("PUT", "/apps/{name}/api/services/files/{*path}", "a published App's page keeps its own objects under its prefix with the person's edge token (AP-170, API/06 §3); an agent changes an App by a run, never its files"),
("DELETE", "/apps/{name}/api/services/files/{*path}", "a published App's page keeps its own objects under its prefix with the person's edge token (AP-170, API/06 §3); an agent changes an App by a run, never its files"),
("POST", "/apps/{name}/api/services/files/{*path}", "a published App's page keeps its own objects under its prefix with the person's edge token (AP-170, API/06 §3); an agent changes an App by a run, never its files"),
("POST", "/apps/{name}/api/services/email/send", "a published App's page sends mail to people of the organization with the person's edge token (AP-168, API/06 §3); an agent mails nobody"),
("GET", "/mail/unsubscribe", "the confirmation page behind the unsubscribe link of an App's message (AP-168, RFC 8058); the signed link is the credential, there is no caller to give an operation"),
("POST", "/mail/unsubscribe", "a mail client's one-click unsubscribe from an App's messages (AP-168, RFC 8058); the signed link is the credential, there is no caller to give an operation"),
("POST", "/auth/backchannel-logout", "signing out"),
("GET", "/auth/callback", "signing in"),
("GET", "/auth/login", "signing in"),
("POST", "/auth/logout", "signing out"),
("GET", "/auth/me", "who is signed in"),
("GET", "/auth/sso-check", "whether the realm session lives, asked silently for a framed App (T-3034)"),
("GET", "/auth/sso-check/done", "the empty page that silent check lands on (T-3034)"),
("GET", "/blueprints", "the organisation's gallery, not a project's data; jc_flow_start runs one by name"),
("GET", "/branding", "the instance's look, not a project's data"),
("GET", "/branding/{asset}", "the instance's look, not a project's data"),
("GET", "/catalogue", "the public open-data catalogue, read from CKAN as an anonymous caller (EP-81): not a project's data"),
("GET", "/organization/data-access", "the Organization's Data access matrix, decided per Endpoint and column by the gateway (access/simulate); it writes nothing, a change is a Policy proposal (T-6412)"),
("GET", "/organization/people/{id}/readable", "what one person can read as the gateway decides it, Endpoint names only; it writes nothing (T-6412)"),
("GET", "/catalogue/readable", "the catalogue page's list of what the caller may read, decided per Endpoint by the gateway (access/simulate); it writes nothing, and an agent asks the gateway's MCP hub, which decides the same way (T-6411)"),
("GET", "/catalogue/datasets/{name}", "the public open-data catalogue, read from CKAN as an anonymous caller (EP-82): not a project's data"),
("GET", "/catalogue/datasets/{name}/sample", "a public Endpoint read anonymously, as any visitor of the catalogue reads it (EP-82)"),
("GET", "/catalogue/datasets/{name}/map", "a public Endpoint read anonymously, as any visitor of the catalogue reads it (EP-82, T-5628)"),
("GET", "/catalogue/endpoints/{slug}/standards", "a public Endpoint read anonymously, as any visitor of its page reads it (TS-19)"),
("GET", "/forms", "the form definitions the Portal renders, not a project's data"),
("GET", "/kinds/{kind}/schema", "a manifest kind's published schema, not a project's data"),
("GET", "/health", "liveness"),
("POST", "/internal/agent-runs/events", "the runner's own callback, authenticated as a workload"),
("GET", "/internal/agent-runs/{id}", "the runner's own callback, authenticated as a workload"),
("GET", "/internal/agent-runs/{id}/diagnostics/{component}/{name}", "the runner's own callback, authenticated as a workload"),
("POST", "/internal/model-key", "the agent proxy reports the model key's state, authenticated as a workload (AG-96)"),
("POST", "/internal/pipelines/{project}/{name}/samples", "the pipeline runner's taps post a sample of what crossed a node, authenticated as a workload (T-3974)"),
("POST", "/internal/pipelines/{project}/{name}/outcomes", "the pipeline runner's outcome sink posts the records of a batch the gateway took, authenticated as a workload (PL-62)"),
("POST", "/internal/pipelines/{project}/{name}/source-deletes", "a snapshot pipeline's delete stream posts what the gateway deleted, found gone or refused, authenticated as the pipeline's workload (PL-97)"),
("POST", "/internal/pipelines/{project}/{name}/source-events", "an events pipeline's stream posts the ids its deletedWhen marked and the records it could not judge, authenticated as the pipeline's workload (PL-98)"),
("POST", "/internal/pipelines/{project}/{name}/rejected", "the pipeline runner's validation stage posts a refused record, authenticated as a workload (PL-61)"),
("GET", "/internal/agent-runs/{id}/inbox", "the runner's own callback, authenticated as a workload"),
("POST", "/internal/agent-runs/{id}/mcp", "the whole registry for one run, narrowed by its AgentProfile (AG-70) and refused an approval (AG-11)"),
("POST", "/internal/pipeline-tests/{id}", "the runner's own callback, authenticated as a workload"),
("GET", "/internal/domain-verifications", "the gateway's read of each Organization's domain state, authenticated as a workload"),
("GET", "/internal/previews", "the gateway's read of the running previews, authenticated as a workload"),
("GET", "/internal/revocations", "the gateway's read of the revoked subjects, authenticated as a workload"),
("GET", "/internal/connectors/{sub}", "the gateway's read of one person's MCP connectors, authenticated as a workload (EP-123)"),
("POST", "/internal/usage", "the gateway replicas post their call counts per Endpoint and surface, authenticated as a workload (T-5118)"),
("GET", "/mcp", "the MCP door itself, which dispatches this registry"),
("POST", "/mcp", "the MCP door itself, which dispatches this registry"),
("GET", "/metrics", "the Prometheus scrape"),
("POST", "/live-notify/{key}", "the gateway's delivery of the Portal's own live-update notifications: a broker calls it, nobody else (API/01 §32)"),
("GET", "/openapi.json", "the API document"),
("GET", "/organization/health-checks", "the administrator's Health page across projects (T-5067); an agent reads one resource's status with jc_resource_get"),
("GET", "/organization/health", "the validation checks' published results, about the installation and not a project's data (OPS-53)"),
("GET", "/organization/agents", "a summary for the Agents page, read from the manifests jc_resource_list already reaches and the caller's own run list; it writes nothing (T-4849, API/01 §48)"),
("GET", "/organization/agents/activity", "a summary for an agent's Activity tab, read from the runs jc_run_list already reaches and their events; it writes nothing (T-4851, API/01 §49)"),
("GET", "/organization/costs", "a summary for the Costs page: the operator's price table split by the space bytes and assistant spend the Portal already reads; it writes nothing (T-5126)"),
("GET", "/organization/costs/export", "the Costs page's month as a CSV download; it writes nothing (T-5126)"),
("GET", "/organization/brokers", "a summary for the Brokers page: each broker's health and the counts of the spaces jc_resource_get already reaches; it writes nothing (T-3990, API/01 §43)"),
("GET", "/projects/{project}/mqttbrokers/{name}/access", "explains which roles of a broker one ServiceAccount holds now for the broker card, jc_core::mqtt::roles_held over the two manifests jc_resource_get already reaches; it writes nothing (T-5876, API/01 §47)"),
("GET", "/projects/{project}/mqttbrokers/{name}/bindings", "a join for the broker card of the DataSource, Pipeline and Endpoint manifests jc_resource_get already reaches, each hop as the caller may read it; it writes nothing (T-5533, API/01 §47)"),
("GET", "/projects/{project}/mqtt/allowance", "a sum for the MQTT page of the Organization and Project manifests and the MqttBrokers jc_resource_get already reaches; it writes nothing (T-4859, API/01 §47)"),
("GET", "/projects/{project}/mqttbrokers/{name}/devices", "the Add device form's role and topic options for the brokers page; it writes nothing and an agent reads the broker with jc_resource_get (T-5532)"),
("POST", "/projects/{project}/mqttbrokers/{name}/devices", "a person adds a device whose key pair the browser made; an agent proposes the ServiceAccount and the MqttBroker with jc_resource_propose (T-5532)"),
("GET", "/projects/{project}/mqttbrokers/{name}/captured", "jc_mqtt_captured"),
("DELETE", "/projects/{project}/mqttbrokers/{name}/captured", "a steward deletes a capture at once; no agent tool deletes what a person may need to see first (T-4880)"),
("GET", "/projects/{project}/mqttbrokers/{name}/measured", "what an MQTT broker measures, read from its own HTTP API for the brokers page; it writes nothing and an agent reads the broker with jc_resource_get (T-4043, API/01 §44)"),
("POST", "/projects/{project}/mqttbrokers/{name}/test-publish", "a dry run of one device message for the broker card's Test drawer, publishing and writing nothing (T-5534, API/01 §44a); jc_mqtt_test_publish answers the same (JC2-59)"),
("GET", "/projects/{project}/mqttbrokers/{name}/watch", "the gateway's refusals of a broker and its measured connections for the Test drawer's watch; it writes nothing (T-5534, API/01 §44a)"),
("GET", "/organization/limits", "a summary for the settings page: the catalog and the operator's bounds beside the values the Organization and Project manifests hold, which jc_resource_get reaches; it writes nothing (T-2715, API/01 §28)"),
("GET", "/organization/setup", "a summary for the setup page, read from manifests the operations registry already reaches and from the deployment; it writes nothing (T-2748, API/01 §25)"),
("GET", "/projects/{project}/endpoints/{name}/quality", "the dataset's quality score, a reading about the data the Endpoint page shows and not a manifest (T-5112)"),
("POST", "/projects/{project}/endpoints/{name}/preview", "the filter editor's proof, one page through a draft a person is typing (EP-85); jc_endpoint_preview answers the same for a client (JC2-33)"),
("POST", "/projects/{project}/kpis/check", "the KPI editor's check as a person types, and its preview on the sources' current pages; the assistant proposes a KPI and the proposal's dry run holds it (PL-105)"),
("GET", "/projects/{project}/endpoints/{name}/access/matrix", "the Endpoint page's Access tab: who may do what, from the gateway's grant documents; no tool asks it (API/01 §57, EP-103)"),
("POST", "/projects/{project}/access/explore", "an administrator's look at what another principal reaches, from the Access explorer (API/01 §41, EP-103); jc_access_explore answers the same (JC2-33)"),
("GET", "/projects/{project}/serviceaccounts/{name}/access", "an agent account's grants on its Access view; no tool asks it (API/01 §52, AG-130)"),
("POST", "/projects/{project}/serviceaccounts/{name}/revoke", "a person revokes an agent's grant at once in the Portal; an agent never changes access (API/01 §52, AG-132)"),
("GET", "/projects/{project}/endpoints/{name}/access/attributes", "the cell inspector names the Policy behind a refused attribute for its own caller (API/01 §58); jc_attribute_explain answers the same (JC2-33)"),
("POST", "/projects/{project}/serviceaccounts/{name}/secret", "a person issues an account's client secret once in the Portal; no tool ever sees a secret (PF-37, T-5626)"),
("GET", "/projects/{project}/secrets", "the secret form lists names and keys, never a value, for a person; no tool may carry a credential (MF-24, T-5971)"),
("POST", "/projects/{project}/secrets", "a person types a credential into the Portal's secret form; no tool may carry one (MF-24, T-5971)"),
("POST", "/projects/{project}/endpoints/{name}/access/simulate", "an administrator's question from the Policies page about another person's rights (EP-103); jc_access_simulate answers the same (JC2-33)"),
("GET", "/organization/people", "jc_person_list"),
("POST", "/organization/people", "jc_person_create"),
("GET", "/organization/people/{id}", "jc_person_get"),
("PATCH", "/organization/people/{id}", "jc_person_edit"),
("DELETE", "/organization/people/{id}", "deleting a person is a Change that takes them out of every Group and RoleBinding, proposed by a person on the page (PF-94)"),
("POST", "/organization/people/remove", "removing a department is one Change that takes them all out of every Group and RoleBinding, proposed by a person on the page (PF-94, T-5606)"),
("GET", "/organization/people/names", "a reviewer's page puts names beside the person ids a manifest holds; a tool reads the ids alone (ADR-N-061)"),
("POST", "/organization/people/name-by-id", "rewriting a repository's people from e-mails to ids is a Change an administrator proposes on the People page, never a tool (ADR-N-061)"),
("POST", "/organization/people/{id}/disable", "jc_person_disable"),
("POST", "/organization/people/{id}/enable", "jc_person_enable"),
("POST", "/organization/people/{id}/remove-second-factor", "removing a second factor takes away a way in; a person does it at the Portal, never a tool (PF-93, AG-11)"),
("POST", "/organization/people/{id}/reset-password", "a reset answers a temporary password, which never reaches a tool's answer (PF-92, AG-11)"),
("POST", "/organization/people/{id}/resend-invitation", "a resend may answer a temporary password, which never reaches a tool's answer (PF-92, PF-108, AG-11)"),
("POST", "/organization/people/{id}/sign-out", "jc_person_sign_out"),
("DELETE", "/organization/people/{id}/consents/{client}", "revoking a consent takes away a person's connection; an administrator does it on the Access explorer, never a tool (API/01 §41)"),
("GET", "/connectors", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("POST", "/connectors", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("GET", "/connectors/options", "what a person may put in their own connector: only what they read already (AG-150, T-5951)"),
("GET", "/connectors/{id}", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("PATCH", "/connectors/{id}", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("DELETE", "/connectors/{id}", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("GET", "/connectors/{id}/activity", "a person manages their own connectors in the Portal; a connector never creates, widens or revokes one (AG-151, API/14 §2)"),
("GET", "/connectors/pending", "an administrator decides connector requests above the ceiling in the Portal; no AI client decides one (AG-150, AG-151)"),
("POST", "/connectors/{id}/approve", "an administrator decides connector requests above the ceiling in the Portal; no AI client decides one (AG-150, AG-151)"),
("POST", "/connectors/{id}/tab", "the person signed in in a tab pairs it with their own connector, whose client steers it with UI actions that never save (T-5967, API/14 §2.1)"),
("DELETE", "/connectors/{id}/tab", "the person signed in in a tab pairs it with their own connector, whose client steers it with UI actions that never save (T-5967, API/14 §2.1)"),
("GET", "/connectors/{id}/tab/{tab}/events", "the person signed in in a tab pairs it with their own connector, whose client steers it with UI actions that never save (T-5967, API/14 §2.1)"),
("GET", "/organization/connectors", "an administrator reviews every person's connectors in the Portal; a connector never reads or revokes one (AG-151, T-5975)"),
("DELETE", "/organization/connectors/{id}", "an administrator reviews every person's connectors in the Portal; a connector never reads or revokes one (AG-151, T-5975)"),
("GET", "/c/{id}", "a connector's MCP door, which dispatches this registry narrowed by the connector (AG-143)"),
("POST", "/c/{id}", "a connector's MCP door, which dispatches this registry narrowed by the connector (AG-143)"),
("GET", "/c/{id}/.well-known/oauth-protected-resource", "the OAuth resource metadata an MCP client reads before it asks a token for the connector"),
("GET", "/preferences", "this person's own Portal preferences, not a project's data"),
("GET", "/app-templates", "the Portal's own App templates, the same for every signed-in person, no project's data"),
("GET", "/app-templates/{name}/screenshot/{width}", "a picture of a Portal App template, the same for every signed-in person, no project's data"),
("GET", "/visualization-templates/{name}", "a Portal chart template filled with the columns given, the same for every signed-in person, no project's data"),
("PUT", "/preferences", "this person's own Portal preferences, not a project's data"),
("POST", "/preferences/recent", "this person's own recent pages, not a project's data"),
("GET", "/endpoints", "jc_endpoint_list_all"),
("GET", "/projects", "the door before a project; every operation runs inside one, and the MCP door's initialize instructions name the projects the caller reads (JC2-29)"),
("POST", "/projects", "jc_project_create"),
("GET", "/projects/{project}", "jc_project_get"),
("DELETE", "/projects/{project}", "jc_project_delete"),
("POST", "/projects/{project}/duplicate", "copies a whole repository of the forge under a new slug (PF-89): a person's decision in the Portal, not a tool a run holds"),
("GET", "/projects/{project}/flow-map", "the data flow map the Pipelines page draws from the mirror; an agent reads the same manifests with jc_resource_list and runs with jc_pipeline_metrics (T-4862)"),
("GET", "/projects/{project}/registry", "what the organization's registry entry pins the project to, read for the Release section's form; jc_project_get answers the project itself (PF-86)"),
("PUT", "/projects/{project}/registry", "repointing what a project runs is the organization's red-lane decision a person proposes in the Portal, not a tool a run holds (PF-86, PF-58)"),
("GET", "/projects/{project}/health-checks", "the project's Health page (T-5067): what is broken and how to fix it; an agent reads one resource's status with jc_resource_get"),
("GET", "/projects/{project}/endpoints/{name}/usage", "the publisher's Usage tab (T-5118): calls per day the gateway counted, not a manifest"),
("GET", "/projects/{project}/usage/endpoints", "the project's Usage page (T-5118): calls per Endpoint the gateway counted, not a manifest"),
("GET", "/projects/{project}/usage/endpoints/export", "the monthly usage CSV a person downloads (T-5118), not a manifest"),
("GET", "/projects/{project}/app-checks", "the App probe's published verdicts, about the installation's checks and not a manifest (AP-136)"),
("GET", "/projects/{project}/activity", "jc_activity_list"),
("POST", "/projects/{project}/catalogue/drafts", "drafts the publish form and writes nothing; jc_catalogue_draft answers the same, and the proposal is the Endpoint's own (JC2-33)"),
("GET", "/projects/{project}/activity/stream", "a live stream, not a call and an answer"),
// Drift is read and resolved on the page, not through the assistant (CC-21, UI-26): a
// resolution is a write to the live space or to the repository, and a model proposing one
// would be acting on a comparison it cannot see. Both are held to `propose` on `Entity`.
("GET", "/projects/{project}/drift", "read on the page; jc_space_governance aspect drift answers it per space (JC2-60)"),
("POST", "/projects/{project}/drift/{space}/{id}/revert", "a resolution a person picks, held to propose on Entity"),
("POST", "/projects/{project}/drift/{space}/{id}/adopt", "a resolution a person picks, held to propose on Entity"),
("GET", "/projects/{project}/agent-runs", "jc_run_list"),
("POST", "/projects/{project}/agent-runs", "jc_run_create"),
("POST", "/projects/{project}/app-templates/{name}/apps", "a person starts an App from a gallery template as it ships, held to propose on App and reviewed as a Change; an agent builds an App with jc_run_create (T-5617)"),
("GET", "/projects/{project}/agent-runs/{id}", "jc_run_get"),
("POST", "/projects/{project}/agent-runs/{id}/answers", "jc_run_answer"),
("POST", "/projects/{project}/agent-runs/{id}/cancel", "jc_run_cancel"),
("GET", "/projects/{project}/agent-runs/{id}/events", "a live stream, not a call and an answer"),
("POST", "/projects/{project}/agent-runs/{id}/functions/{fn}", "the run calling its own tools back through the Portal"),
("POST", "/projects/{project}/agent-runs/{id}/messages", "jc_run_message"),
("GET", "/projects/{project}/agent-runs/{id}/preview", "the page that frames a run's preview, not an answer"),
("POST", "/projects/{project}/agent-runs/{id}/preview-errors", "the preview frame reporting its own errors"),
("POST", "/projects/{project}/agent-runs/{id}/preview-observations", "the preview frame reporting what it sees"),
("POST", "/projects/{project}/agent-runs/{id}/publish", "jc_run_publish"),
("GET", "/projects/{project}/apps/{name}/build", "links of the App page to the forge's own repository, run and package pages (AP-103); what they show is read in the forge"),
("GET", "/projects/{project}/apps/{name}/audience", "how many people each visibility reaches, for the Share dialog (AP-173); an agent proposes a visibility with jc_resource_propose and reads no head counts"),
("GET", "/shared-apps", "the Portal home's Shared with me (AP-174), about the person signed in; an agent opens no App as a person"),
("GET", "/projects/{project}/apps/{name}/me", "a fullstack App's backend asks for the caller's roles with the edge token (AP-109); an agent acts as itself, never as a person in an App"),
("POST", "/projects/{project}/apps/rename-shapes", "a person's one click on the Apps page that renames every old-shape App of a project in one Change (AP-124, T-2940); an agent or MCP client makes the same rewrite through jc_resource_propose, App by App"),
("POST", "/projects/{project}/apps/{name}/rebuild", "a person asks the forge to run the App's reviewed build.yml again (AP-103); an agent changes an application by a run, which the workflow builds on merge"),
("GET", "/projects/{project}/apps/{name}/builds", "the App page's list of its earlier successful builds on the forge, read beside its build links (AP-171)"),
("POST", "/projects/{project}/apps/{name}/restore", "a person opens a merge request bringing the App's repository back to an earlier build (AP-171); an agent changes an application by a run, which the workflow builds on merge"),
("GET", "/projects/{project}/apps/{name}/dashboard", "the App page's dashboard editor reads the App's src/dashboard.json and the attributes its dropdowns offer (T-6402); an agent reads the App's source in its run"),
("POST", "/projects/{project}/apps/{name}/dashboard", "a person's no-code edit of a dashboard App, proposed as the App's Change (T-6402); an agent changes an application by a run, which proposes the same Change"),
("GET", "/projects/{project}/assistant/access", "what the assistant may reach here; the registry's own listing answers the same question"),
("POST", "/projects/{project}/assistant/conversations", "the assistant's own door; jc_run_create starts a conversation"),
("POST", "/projects/{project}/assistant/propose-endpoint", "jc_endpoint_propose"),
("GET", "/projects/{project}/basemap/{style}/style.json", "map tiles the browser fetches"),
("GET", "/projects/{project}/basemap/{style}/{z}/{x}/{tile}", "map tiles the browser fetches"),
("GET", "/projects/{project}/changes", "jc_change_list"),
("GET", "/projects/{project}/changes/history", "jc_change_history"),
("GET", "/projects/{project}/changes/history/export", "a person or auditor downloads the closed changes under the history filter as CSV from the Approvals history (T-5114)"),
("GET", "/projects/{project}/changes/{id}", "jc_change_get"),
("POST", "/projects/{project}/changes/{id}/approve", "jc_change_approve"),
("POST", "/projects/{project}/changes/{id}/reject", "jc_change_reject"),
("POST", "/feedback", "a person tells the platform's owners what got in their way from the page they are on (T-3272); an agent has no page and no feedback of its own"),
("GET", "/organization/feedback", "an administrator reads what people sent before it becomes a proposed task (T-3272); untrusted text is no agent's input"),
("GET", "/organization/feedback/{id}/screenshot", "an administrator looks at what a person saw (T-3272); a screenshot of a person's page is no agent's input"),
("POST", "/projects/{project}/changes/{id}/restore", "a person brings back what a merged removal took from the Approvals history (T-3247); an agent proposes the manifest again with jc_resource_propose"),
("POST", "/projects/{project}/changes/{id}/undo", "a person puts back what a merged update altered from the Approvals history (T-3274); an agent proposes the earlier manifest with jc_resource_propose"),
("GET", "/projects/{project}/changes/{id}/manifests", "a person loads a version a merged change wrote into the resource's editor to restore it (T-3975); an agent reads the change with jc_change_get and proposes the manifest with jc_resource_propose"),
("GET", "/projects/{project}/pipelinetemplates", "a person picks a saved pipeline in the gallery to start from (T-3976); an agent writes a pipeline with jc_resource_propose and needs no gallery"),
("POST", "/projects/{project}/pipelinetemplates", "a person saves a pipeline as a template of the project from its editor (T-3976); an agent keeps no gallery of its own"),
("DELETE", "/projects/{project}/pipelinetemplates/{id}", "a person removes a template from the gallery (T-3976); an agent keeps no gallery of its own"),
("GET", "/projects/{project}/ckan/status", "jc_ckan_status"),
("GET", "/organization/datamodels", "the list behind the model and type pickers (DM-63); an agent reads a project's models through jc_resource_list and the catalogue through jc_catalog_search"),
("GET", "/projects/{project}/datamodels/{name}/source", "jc_model_source_get"),
("PUT", "/projects/{project}/datamodels/{name}/source", "jc_model_source_put"),
("POST", "/projects/{project}/datamodels/{name}/share", "the model page's Share with the organization (DM-77): a red-lane Change an organization administrator approves in the Portal; an agent drafts the model, a person decides to share it"),
("GET", "/projects/{project}/drafts", "jc_draft_list"),
("GET", "/projects/{project}/drafts/events", "a live stream, not a call and an answer"),
("GET", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_get"),
("PUT", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_put"),
("DELETE", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_drop"),
("GET", "/projects/{project}/export", "jc_project_export"),
("GET", "/projects/{project}/apps/{name}/export", "the Administration page's download of one App as a zip of its git bundle and manifest (UI-87), an organization administrator's in a browser; an agent reads the App with jc_resource_get"),
("POST", "/projects/{project}/flows", "jc_flow_start"),
("POST", "/projects/{project}/import", "jc_project_import"),
("GET", "/projects/{project}/ops", "the registry's own listing"),
("POST", "/projects/{project}/ops/{name}", "the registry's own door"),
("GET", "/projects/{project}/permissions/me", "what this caller may do, for the Portal UI; jc_permissions_me answers the same rules (JC2-29)"),
("POST", "/projects/{project}/pipelines/test", "jc_pipeline_test"),
("POST", "/projects/{project}/datasources/{name}/test", "a steward tests a saved data source's connection from its page (T-4819); an agent checks a draft source with jc_resource_propose and dryRun, whose probe fetches an http source"),
("GET", "/projects/{project}/pipelines/{name}/metrics", "jc_pipeline_metrics"),
("GET", "/projects/{project}/pipelines/{name}/samples", "a person opens a node of a running pipeline in the studio (T-3974); jc_pipeline_inspect view samples answers the same (JC2-32)"),
("GET", "/projects/{project}/pipelines/{name}/bento", "the editor shows the bento.yaml a pipeline runs when it has no compute step (T-4818); an agent reads the same file through the forge, and a proposal writes it as a file beside the manifest"),
("GET", "/projects/{project}/pipelines/{name}/stats", "the canvas' live counts per node (PL-68); jc_pipeline_inspect view stats answers the same (JC2-32)"),
("GET", "/projects/{project}/pipelines/{name}/rejected", "the pipeline page's rejected list (PL-61, ADR-N-034); jc_pipeline_inspect view rejected answers the same pages (JC2-32)"),
("POST", "/projects/{project}/pipelines/{name}/rejected/retry", "a person replays the records they picked on the pipeline page once the fix merged (PL-61); an agent changes the mapping with jc_resource_propose and lets the next run bring them"),
("GET", "/projects/{project}/pipelines/{name}/runs", "the pipeline page's runs (PL-62); an agent reads them with their newest log in one call, jc_pipeline_logs (T-6028)"),
("POST", "/projects/{project}/pipelines/{name}/runs", "jc_pipeline_run"),
("GET", "/projects/{project}/pipelines/{name}/source-deletes", "the pipeline page's dry run of a snapshot pipeline's source deletes (PL-97); jc_pipeline_inspect view sourceDeletes answers the same (JC2-32)"),
("POST", "/projects/{project}/pipelines/{name}/source-deletes", "a steward's answer to a held run, delete anyway or keep (PL-97): a person's decision over deletes, never an agent's; an agent proposes a changed spec.sourceDeletes with jc_resource_propose"),
("GET", "/projects/{project}/pipeline-health", "the pipelines page's Health tab (T-5048): a steward sees which sources stopped delivering; an agent reads one pipeline with jc_pipeline_metrics"),
("GET", "/projects/{project}/pipeline-runs", "the pipelines list's Last run column (T-5818): one read for every row; an agent reads one pipeline with jc_pipeline_metrics"),
("GET", "/projects/{project}/pipelines/{name}/runs/{run}/log", "the pipeline page's log of one run (PL-62); an agent pages the same log with jc_pipeline_logs (T-6028)"),
("POST", "/projects/{project}/pipelines/{name}/access-requests", "a steward asks another space's owner for read access from the pipeline page (PL-90); a grant across spaces is a person's decision, so no agent files one"),
("GET", "/projects/{project}/pipelines/{name}/access-requests/endpoints", "the request form's Endpoint, type and attribute choices (PL-90, T-4946); a grant across spaces is a person's decision, so no agent files one"),
("GET", "/projects/{project}/pipelines/{name}/access-requests", "the pipeline page lists what it asked for and each answer (PL-90); an agent has no request of its own to follow"),
("GET", "/projects/{project}/endpoints/{name}/access-requests", "the Endpoint page shows a person their own requests and the types they may ask for (EP-55); an agent asks for nothing of its own"),
("POST", "/projects/{project}/endpoints/{name}/access-requests", "a person asks the space's owners for a read on the Endpoint page (EP-55, PF-58); a grant to a person is that person's own ask, never an agent's"),
("GET", "/projects/{project}/access-requests", "the owner's queue of requests other spaces filed against its Endpoints (PL-91); deciding them is a person's call"),
("POST", "/projects/{project}/access-requests/{id}/approve", "an owner turns a request into a proposed Change in the Portal (PL-91); widening access across spaces stays with a person, never an agent"),
("POST", "/projects/{project}/access-requests/{id}/decline", "an owner declines a request in the Portal (PL-91); a person answers a person"),
("POST", "/projects/{project}/access-requests/{id}/revoke", "an owner proposes ending a grant in the Portal (PL-93); an agent never changes who may read another space"),
("GET", "/projects/{project}/revisions", "jc_project_revisions"),
("GET", "/projects/{project}/spaces/{space}/quality", "the daily data-quality report, a reading about the data and not a manifest (DM-74); jc_space_governance aspect quality answers it (JC2-60)"),
("GET", "/projects/{project}/spaces/{space}/quality/checks", "the checks on write by rule over 24 hours and 7 days, counters and not a manifest (DM-94); jc_space_governance aspect qualityChecks answers it (JC2-60)"),
("GET", "/projects/{project}/spaces/{space}/live", "which entities of a type changed, by name, as server-sent events for an open data view: a stream of hints read with the space's read rule, not a manifest (API/01 §32, T-3105)"),
("GET", "/projects/{project}/knowledge/sources", "what the knowledge assistant holds of the project's sources: jc-assistant's store beside the KnowledgeSource manifests (API/01 §34, T-3057)"),
("GET", "/projects/{project}/knowledge/sources/{source}/pages", "one level of a crawled source's page tree, from jc-assistant's store (API/01 §34)"),
("GET", "/projects/{project}/knowledge/sources/{source}/documents", "the documents a crawled source's pages link, from jc-assistant's store (API/01 §34)"),
("GET", "/projects/{project}/knowledge/sources/{source}/search", "what search_documents would answer from one source, from jc-assistant's store (API/01 §34, UI-99)"),
("GET", "/projects/{project}/knowledge/sources/{source}/errors", "the failed pages and documents of a source with why, for proposers (API/01 §34, T-4197)"),
("GET", "/projects/{project}/knowledge/sources/{source}/pages/{page}/links", "the links of one crawled page, from jc-assistant's store (API/01 §34)"),
("GET", "/projects/{project}/knowledge/sources/{source}/passages", "the passages the assistant answers from, of one page or document (API/01 §34)"),
("POST", "/projects/{project}/knowledge/sources/{source}/inclusion", "an administrator's include or exclude of crawled pages and documents: runtime state of jc-assistant, not configuration (API/01 §34, AG-113)"),
("POST", "/projects/{project}/knowledge/sources/{source}/recrawl", "queues a crawl now in jc-assistant's queue (API/01 §34)"),
("GET", "/projects/{project}/knowledge/sources/{source}/files", "the files uploaded to a source: upload, from jc-assistant's store (API/01 §34, T-4184)"),
("POST", "/projects/{project}/knowledge/sources/{source}/files", "an administrator's file, read into a source: upload's passages by jc-assistant (API/01 §34, T-4184)"),
("DELETE", "/projects/{project}/knowledge/sources/{source}/files/{id}", "removes an uploaded file and its passages in jc-assistant (API/01 §34, T-4184)"),
("GET", "/projects/{project}/knowledge/deployments/{deployment}/usage", "an assistant deployment's requests and tokens per day, from jc-assistant's store (API/01 §34)"),
("DELETE", "/projects/{project}/knowledge/deployments/{deployment}/answers", "forgets the answers an assistant deployment keeps for repeated questions, in jc-assistant's memory (API/01 §34, T-4731)"),
("GET", "/projects/{project}/knowledge/deployments/{deployment}/context", "what a saved assistant reads, resolved from the mirrored manifests by jc-core's resolve_connectors: a read of configuration, not a write (API/05 §1.9, AG-121, T-4693)"),
("POST", "/projects/{project}/knowledge/deployments/context", "the editor's preview of an unsaved AssistantDeployment, dryRun=All: resolves and changes nothing, the save is the AssistantDeployment's own proposal (API/05 §1.9, AG-121, T-4693)"),
("POST", "/projects/{project}/knowledge/deployments/{deployment}/chat", "one question to an assistant as the signed-in person, answered by jc-assistant as events: a conversation, not configuration (API/01 §34, AG-115)"),
("GET", "/projects/{project}/knowledge/conversations", "the stored assistant conversations, redacted, for the assistant's administrator: jc-assistant's runtime record, not configuration (API/01 §34, T-4670)"),
("GET", "/projects/{project}/knowledge/conversations/usage", "what the stored assistant turns add up to, computed by jc-assistant for the assistant's administrator (API/01 §34, T-4728)"),
("GET", "/projects/{project}/knowledge/conversations/export", "the stored assistant turns as CSV, written by jc-assistant for the assistant's administrator (API/01 §34, T-4728)"),
("GET", "/projects/{project}/knowledge/conversations/{conversation}", "one stored conversation with its steps, from jc-assistant's store (API/01 §34, T-4670)"),
("DELETE", "/projects/{project}/knowledge/conversations/{conversation}", "erases one stored conversation in jc-assistant for a visitor's request (API/01 §34, T-4670)"),
("GET", "/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same", "the stored turns of one question, from jc-assistant's store, to compare two answers (API/01 §34, T-4681)"),
("GET", "/projects/{project}/spaces/{space}/source-removals", "what snapshot pipelines deleted from a space because their source dropped it, ids and reasons only (T-4892, PL-100); an agent reads a pipeline's report with jc_pipeline_metrics"),
("GET", "/projects/{project}/spaces/{space}/trash", "the caller's own copies of what they deleted from a data view: a person's record in the Portal's database, not a manifest (API/01 §31, T-3107)"),
("POST", "/projects/{project}/spaces/{space}/trash", "keeps the caller's copy of an entity they are about to delete through the gateway: the delete itself is their gateway write (API/01 §31)"),
("DELETE", "/projects/{project}/spaces/{space}/trash/{id}", "forgets one of the caller's copies after a restore or a refused delete (API/01 §31)"),
("GET", "/projects/{project}/spaces/{space}/comments", "the comments on one entity of a data view: people's notes in the Portal's database, not a manifest (API/01 §35, T-3106)"),
("POST", "/projects/{project}/spaces/{space}/comments", "a person comments on an entity and mentions colleagues: a note in the Portal's database that changes no data (API/01 §35)"),
("DELETE", "/projects/{project}/spaces/{space}/comments/{id}", "a person removes their own comment (API/01 §35)"),
("GET", "/notifications", "the signed-in person's notifications of mentions (API/01 §35)"),
("POST", "/notifications/{id}/read", "the signed-in person marks a notification read (API/01 §35)"),
("GET", "/projects/{project}/grafana", "a person's page of saved visualizations; its rows are dashboards in Grafana, not manifests (T-6509)"),
("GET", "/projects/{project}/grafana/options", "what the visualization form offers one person (T-6509)"),
("POST", "/projects/{project}/grafana", "a person saves a visualization; the Portal writes it into Grafana, no AI tool does (T-6509)"),
("DELETE", "/projects/{project}/grafana/{id}", "a person removes a saved visualization (T-6509)"),
("GET", "/alerts", "the signed-in person's alert subscriptions (API/01 §37)"),
("PUT", "/alerts", "the signed-in person subscribes to a pipeline's, a space's or a type's alerts (API/01 §37)"),
("DELETE", "/alerts/{id}", "the signed-in person stops one of their alerts (API/01 §37)"),
("POST", "/alerts/{id}/mute", "the signed-in person mutes or unmutes one of their alerts (API/01 §37)"),
("GET", "/alerts/notices", "the signed-in person's alert notices (API/01 §37)"),
("POST", "/alerts/notices/{id}/read", "the signed-in person marks an alert notice read (API/01 §37)"),
("GET", "/projects/{project}/spaces/{space}/move", "the state of a move an administrator started; reading it writes nothing (T-4000, API/01 §45)"),
("POST", "/projects/{project}/spaces/{space}/move", "an administrator's operation on the broker that holds a space, not a manifest write: the job commits the hold and the switch itself, and jc_resource_apply of a brokerRef is refused for a space with entities (T-4000, PF-115)"),
("POST", "/projects/{project}/spaces/{space}/retention/preview", "what a draft of the space's retention would delete now, counted by the broker for the space's owner before saving: a reading about the data, the rules themselves are the manifest's (T-3901, DM-85)"),
("POST", "/projects/{project}/spaces/{space}/subjects/export", "a data subject's Entities with their history, for the space's owner, the subject in the body and never the URL (T-4668, DM-124)"),
("GET", "/projects/{project}/mcp-elicitations/{id}", "the question an MCP call asked, for the person whose client asked it (T-5699, AG-63)"),
("POST", "/projects/{project}/mcp-elicitations/{id}/decision", "the person accepts or declines an MCP call's question in their own session (T-5699, AG-63)"),
("GET", "/mcp-confirmations/{id}", "the question a broker's MCP write asked, read from the broker for the person whose client asked it (T-5829)"),
("POST", "/mcp-confirmations/{id}/decision", "the person accepts or declines a broker MCP write in their own session, recorded on the broker in the session subject's name (T-5829)"),
("POST", "/projects/{project}/spaces/{space}/erasures", "a data subject's erasure across the broker, CKAN, the Portal's copies and the stored conversations, logged by keyed hash (T-4668, DM-126)"),
("GET", "/projects/{project}/spaces/{space}/erasures/{id}", "one erasure's state per store, for the space's owner (T-4668, DM-127)"),
("GET", "/projects/{project}/spaces/{space}/erasures", "the space's erasures, proposals waiting for a second owner included (T-5356, DM-142)"),
("POST", "/projects/{project}/spaces/{space}/erasures/{id}/approve", "a second owner approves a proposed erasure with the subject typed again (T-5356, DM-142)"),
("POST", "/projects/{project}/spaces/{space}/erasures/{id}/decline", "an owner declines a proposed erasure with a reason (T-5356, DM-142)"),
("GET", "/projects/{project}/spaces/{space}/retention/deleted", "what the broker's retention sweeps deleted of the space, read with the space's own read rule (T-3901, DM-85)"),
("GET", "/projects/{project}/spaces/{space}/usage", "how many entities the space holds, the broker's count read with the space's own read rule: a reading about the data and not a manifest (T-2889); jc_space_governance aspect usage answers it (JC2-60)"),
("POST", "/projects/{project}/spaces/{space}/usage/recalculate", "the Spaces list's Recalculate: the same count measured again for a reader, one broker read however many ask; a reading about the data, which an MCP client reads through its endpoints (T-6208)"),
("GET", "/projects/{project}/spaces/{space}/temporal-audit", "how each pipeline writing the space creates history rows, per type and attribute with a verdict (T-4844, DM-104..DM-109): a reading of the space's manifests for its owner, which an MCP client reads as the pipelines and the space themselves; jc_space_governance aspect temporalAudit answers it (JC2-60)"),
("GET", "/projects/{project}/spaces/{space}/surface-fit", "which domain-standard surfaces the space's data model can serve, for the Endpoint form's checkboxes (EP-125, T-5299): a reading of the model, which an MCP client reads as the model itself; jc_space_governance aspect surfaceFit answers it (JC2-60)"),
("GET", "/projects/{project}/spaces/{space}/types/{type}/attributes", "the pipeline editor's reading of a type's attributes in the space's data model (API/01): a person mapping a record onto a type, which an MCP client does with the model's own JSON Schema on the endpoint"),
("GET", "/projects/{project}/spaces/{space}/views", "the Portal UI's own saved views of a space (API/01 §30): a person's way of looking at entities, not data and not a manifest; MCP clients read the entities through endpoints"),
("POST", "/projects/{project}/spaces/{space}/views", "the Portal UI's own saved views of a space (API/01 §30): a person's way of looking at entities, not data and not a manifest; MCP clients read the entities through endpoints"),
("PUT", "/projects/{project}/spaces/{space}/views/{id}", "the Portal UI's own saved views of a space (API/01 §30): a person's way of looking at entities, not data and not a manifest; MCP clients read the entities through endpoints"),
("DELETE", "/projects/{project}/spaces/{space}/views/{id}", "the Portal UI's own saved views of a space (API/01 §30): a person's way of looking at entities, not data and not a manifest; MCP clients read the entities through endpoints"),
("POST", "/projects/{project}/syncsources/{name}/detach", "jc_syncsource_detach"),
("POST", "/projects/{project}/syncsources/{name}/pause", "jc_syncsource_pause"),
("GET", "/projects/{project}/syncsources/{name}/status", "jc_syncsource_status"),
("POST", "/projects/{project}/syncsources/{name}/sync", "jc_syncsource_sync"),
("GET", "/projects/{project}/workspaces", "jc_workspace_list"),
("POST", "/projects/{project}/workspaces", "jc_workspace_open"),
("GET", "/projects/{project}/workspaces/{name}", "jc_workspace_get"),
("DELETE", "/projects/{project}/workspaces/{name}", "jc_workspace_discard"),
("GET", "/projects/{project}/workspaces/{name}/compare", "jc_workspace_compare"),
("POST", "/projects/{project}/workspaces/{name}/update", "jc_workspace_update_from_main"),
("POST", "/projects/{project}/workspaces/{name}/propose", "jc_workspace_propose"),
("POST", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_start"),
("GET", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_get"),
("DELETE", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_stop"),
("GET", "/projects/{project}/{plural}", "jc_resource_list"),
("POST", "/projects/{project}/{plural}", "jc_resource_propose"),
("GET", "/projects/{project}/{plural}/{name}", "jc_resource_get"),
("PUT", "/projects/{project}/{plural}/{name}", "jc_resource_propose"),
("PATCH", "/projects/{project}/{plural}/{name}", "jc_resource_propose"),
("DELETE", "/projects/{project}/{plural}/{name}", "jc_resource_delete"),
("GET", "/ready", "readiness"),
("GET", "/version", "the build of the binary and of the UI bundle, nothing of a project (T-2485)"),
("POST", "/tools/generate", "jc_model_generate"),
("POST", "/tools/import-sdm", "jc_model_sdm_import"),
("POST", "/tools/infer-schema", "the model tools the Portal proxies; jc_model_infer is the project's door"),
("GET", "/tools/sdm-catalog", "the wizard's whole catalogue index; jc_model_sdm_search answers the same index narrowed by q (T-6271)"),
("POST", "/webhooks/gitea", "the forge calling in"),
("POST", "/webhooks/sync/{project}/{name}", "a foreign catalogue calling in"),
];

#[test]
fn every_route_is_an_operation_or_says_why_it_is_not() {
    let mut expected: std::collections::HashMap<(String, String), &str> = ROUTE_COVERAGE
        .iter()
        .map(|(method, path, behind)| (((*method).to_owned(), (*path).to_owned()), *behind))
        .collect();
    assert_eq!(
        expected.len(),
        ROUTE_COVERAGE.len(),
        "ROUTE_COVERAGE names the same route twice"
    );

    for (method, path) in routes_in_source() {
        let behind = expected
            .remove(&(method.clone(), path.clone()))
            .unwrap_or_else(|| {
                panic!(
                    "route `{method} {path}` is in no line of ROUTE_COVERAGE: give it an \
                     operation, or say there why it has none"
                )
            });
        if behind.starts_with("jc_") {
            assert!(
                ops::find(behind).is_some(),
                "route `{method} {path}` names operation `{behind}`, which the registry \
                 does not hold"
            );
        } else {
            assert!(
                !behind.trim().is_empty(),
                "route `{method} {path}` has no operation and no reason"
            );
        }
    }

    let mut left: Vec<&(String, String)> = expected.keys().collect();
    left.sort();
    assert!(
        left.is_empty(),
        "ROUTE_COVERAGE names routes the Portal no longer serves: {left:?}"
    );
}

/// MF-34, T-1647…T-1654: a route with an operation behind it is published in that operation's
/// words. The OpenAPI document, the MCP tool list and the assistant each describe the same
/// door, and a person reading any of them reads the same sentence: the summary is the
/// operation's title and the description is its description, so the two cannot drift apart.
#[test]
fn a_route_is_published_in_the_words_of_its_operation() {
    use utoipa::OpenApi;
    let spec = joinedcontext_portal::openapi::ApiDoc::openapi();
    let mut compared = 0;
    let mut differ = Vec::new();
    for (method, path, operation) in ROUTE_COVERAGE {
        let Some(op) = ops::find(operation) else {
            continue;
        };
        let Some(item) = spec.paths.paths.get(&format!("/api/v1{path}")) else {
            continue;
        };
        let published = match *method {
            "GET" => item.get.as_ref(),
            "POST" => item.post.as_ref(),
            "PUT" => item.put.as_ref(),
            "PATCH" => item.patch.as_ref(),
            "DELETE" => item.delete.as_ref(),
            _ => None,
        };
        let Some(published) = published else {
            continue;
        };
        compared += 1;
        let wanted = format!("{}.", op.description.trim_end_matches('.'));
        if published.summary.as_deref() != Some(op.title)
            || published.description.as_deref() != Some(wanted.as_str())
        {
            differ.push(format!("{method} {path} ({operation})"));
        }
    }
    assert!(compared > 40, "only {compared} routes were compared");
    assert!(
        differ.is_empty(),
        "published in other words than their operation's: {differ:?}"
    );
}

#[test]
#[should_panic(expected = "path is not a string literal")]
fn a_route_under_a_constant_path_is_refused_not_skipped() {
    routes_in("Router::new().route(MCP_PATH, post(message))");
}

#[test]
#[should_panic(expected = "method this inventory cannot read")]
fn a_route_on_any_method_is_refused_not_skipped() {
    routes_in(r#"Router::new().route("/hidden", any(handler))"#);
}

#[test]
#[should_panic(expected = "nested under another prefix")]
fn a_router_nested_under_another_prefix_is_refused() {
    routes_in(r#"Router::new().nest("/internal", internal::router())"#);
}

#[test]
#[should_panic(expected = "hides its routes")]
fn a_service_mounted_beside_the_routes_is_refused() {
    routes_in(r#"Router::new().nest_service("/files", ServeDir::new("x"))"#);
}

#[test]
fn a_route_is_read_with_each_method_and_the_api_prefix_off() {
    assert_eq!(
        routes_in(r#"Router::new().nest("/api/v1", api()).route("/api/v1/x", get(a).post(b))"#),
        [
            ("GET".to_owned(), "/x".to_owned()),
            ("POST".to_owned(), "/x".to_owned())
        ]
    );
}
