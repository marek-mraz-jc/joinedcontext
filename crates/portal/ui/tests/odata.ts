import { vi } from "vitest";
import metadata from "../../sdk/odata/org-admin.metadata.json" with { type: "json" };
// The Organization's OData service as a page test meets it (T-6812): which bound function or
// action a request calls, with what, and the answer the service wraps an operation's value in.

/** One call of a bound function or action, as the SDK client sends it. */
export interface ODataCall {
  project: string;
  name: string;
  params: Record<string, unknown>;
}

const BOUND =
  /^\/api\/v1\/projects\('((?:[^']|'')*)'\)\/JoinedContext\.Portal\.(\w+)(?:\(([^)]*)\))?$/;

/** A literal of an alias: text in quotes, a number, a boolean, `null` or JSON. */
function literal(text: string): unknown {
  if (text.startsWith("'") && text.endsWith("'"))
    return text.slice(1, -1).replaceAll("''", "'");
  return JSON.parse(text) as unknown;
}

/**
 * The function or action `request` calls, or nothing for any other request: a function's
 * arguments come from its aliases, an action's from its JSON body (`body`).
 */
export function odataCall(
  url: string | URL,
  body?: unknown,
): ODataCall | undefined {
  const at = new URL(String(url), "http://localhost");
  const bound = BOUND.exec(decodeURIComponent(at.pathname));
  if (!bound) return undefined;
  const params: Record<string, unknown> = {};
  for (const argument of (bound[3] ?? "").split(",").filter(Boolean)) {
    const [name, alias] = argument.split("=");
    const value = at.searchParams.get(alias);
    if (value !== null) params[name] = literal(value);
  }
  if (typeof body === "object" && body !== null) Object.assign(params, body);
  return { project: bound[1].replaceAll("''", "'"), name: bound[2], params };
}

/** The body of the service's answer to a function or action: the operation's value under `value`. */
export function odataBody(value: unknown): string {
  return JSON.stringify({
    "@odata.context": "/api/v1/$metadata#Edm.Untyped",
    value,
  });
}

/** The service's answer to a function or action. */
export function odataValue(value: unknown, status = 200): Response {
  return new Response(odataBody(value), {
    status,
    headers: { "Content-Type": "application/json", "OData-Version": "4.01" },
  });
}

/** The service's refusal: the OData error object with API/09's members inside (API/13 §2a). */
export function odataError(
  status: number,
  code: string,
  message: string,
  extra: Record<string, unknown> = {},
): Response {
  return new Response(
    JSON.stringify({
      error: {
        code,
        message,
        type: `https://joinedcontext.com/errors/${code}`,
        title: "Error",
        status,
        ...extra,
      },
    }),
    { status, headers: { "Content-Type": "application/json" } },
  );
}

/**
 * A REST door's answer as the service gives the same operation's: a success's body under `value`,
 * a problem document as the OData error object carrying its members.
 */
export async function asOData(rest: Response, shape?: (body: unknown) => unknown): Promise<Response> {
  const answer = await asODataBody(rest, shape);
  // The edge's reference of the answer, which a failure page shows (T-2747).
  const id = rest.headers.get("x-request-id");
  if (id) answer.headers.set("x-request-id", id);
  return answer;
}

async function asODataBody(rest: Response, shape?: (body: unknown) => unknown): Promise<Response> {
  // A file a door answers (a pipeline's bento.yaml) is its text as a JSON string (odata_org.rs
  // door_value); a stub's `new Response(json)` says text/plain, so that one stays JSON here.
  const type = rest.headers.get("Content-Type") ?? "";
  const file = rest.ok && type.startsWith("text/") && !type.startsWith("text/plain");
  const body: unknown = file ? await rest.text() : await rest.json().catch(() => null);
  // A `204` of the REST door is the service's `200` with `null` (API/13 §2b).
  if (rest.ok) return odataValue(shape ? shape(body) : body, rest.status === 204 ? 200 : rest.status);
  // A bare failure stays bare: the service passes on a door's answer that said nothing.
  if (body === null) return new Response(null, { status: rest.status });
  const problem = (body ?? {}) as {
    type?: string;
    detail?: string;
    title?: string;
    hint?: string;
    errors?: unknown;
  };
  const code = problem.type?.split("/").pop() ?? "internal-error";
  // As the service's as_odata_error: each field the problem names a detail with its target.
  const details = (Array.isArray(problem.errors) ? problem.errors : []).flatMap(
    (e: unknown) => {
      // A stub's line `path: message` (or a bare path) is the member the server writes from it.
      const line = typeof e === "string" ? e.split(/: (.*)/s) : undefined;
      const { path, message } = (
        line ? { path: line[0], message: line[1] ?? problem.detail } : (e ?? {})
      ) as { path?: unknown; message?: unknown };
      return typeof path === "string" && typeof message === "string"
        ? [{ code, message, target: path }]
        : [];
    },
  );
  const extra: Record<string, unknown> = details.length
    ? { target: details[0].target, details }
    : {};
  if (problem.hint) extra.hint = problem.hint;
  return odataError(
    rest.status,
    code,
    problem.detail ?? problem.title ?? `HTTP ${rest.status}`,
    extra,
  );
}

const READING =
  /^\/api\/v1\/Organization\/JoinedContext\.Portal\.jc_organization_read\(([^)]*)\)$/;

/** The REST door of each Administration reading: the handler `jc_organization_read` runs (T-7098). */
const READING_DOORS: Record<
  string,
  (params: Record<string, string>) => string
> = {
  projects: () => "/api/v1/projects",
  setup: () => "/api/v1/organization/setup",
  health: () => "/api/v1/organization/health",
  healthChecks: () => "/api/v1/organization/health-checks",
  limits: () => "/api/v1/organization/limits",
  brokers: () => "/api/v1/organization/brokers",
  agents: () => "/api/v1/organization/agents",
  agentActivity: () => "/api/v1/organization/agents/activity",
  costs: () => "/api/v1/organization/costs",
  addresses: () => "/api/v1/organization/addresses",
  dataAccess: () => "/api/v1/organization/data-access",
  widgetOptions: () => "/api/v1/organization/widgets/options",
  personReadable: (params) =>
    `/api/v1/organization/people/${encodeURIComponent(params.person ?? "")}/readable`,
};

/**
 * The REST address whose handler answers an Administration reading the page asked the service
 * for, its other members as the query; nothing for any other request. The server proves the two
 * answer alike (odata_org_tests `the_organizations_readings_answer_as_their_rest_doors`), so a
 * page test keeps its REST stub and wraps the stub's answer with `asOData`.
 */
export function readingAsRest(url: string | URL): URL | undefined {
  const at = new URL(String(url), "http://localhost");
  const reading = READING.exec(decodeURIComponent(at.pathname));
  if (!reading) return undefined;
  const params: Record<string, string> = {};
  for (const argument of reading[1].split(",").filter(Boolean)) {
    const [name, alias] = argument.split("=");
    const value = at.searchParams.get(alias);
    if (value !== null) params[name] = String(literal(value));
  }
  const door = READING_DOORS[params.aspect ?? ""];
  if (!door) return undefined;
  const rest = new URL(door(params), at.origin);
  for (const [name, value] of Object.entries(params)) {
    // personReadable's person is its path member; agentActivity's a filter of the query.
    const inPath = name === "person" && params.aspect === "personReadable";
    // agentActivity's inProject is the REST door's project (MCP takes `project` for itself).
    if (name !== "aspect" && !inPath)
      rest.searchParams.set(name === "inProject" ? "project" : name, value);
  }
  return rest;
}

/**
 * Each Portal door's REST door (API/13 §2b, T-7100; crates/portal/src/api/odata_portal.rs), and
 * each registry operation the UI calls whose handler is a REST door's (crates/portal/src/ops):
 * method, path, and the parameters that go in the JSON body (`body` is the whole body).
 */
type Door = [string, string, string[]?];

/**
 * The registry operations every page test reaches as their REST doors (tests/setup.ts): the UI
 * calls them since T-7073, and the files that stub their REST doors keep their stubs.
 */
const OPERATIONS: Record<string, Door> = {
  jc_syncsource_status: ["GET", "/projects/{project}/syncsources/{name}/status"],
  jc_syncsource_sync: ["POST", "/projects/{project}/syncsources/{name}/sync"],
  jc_syncsource_pause: ["POST", "/projects/{project}/syncsources/{name}/pause", ["paused"]],
  jc_ckan_status: ["GET", "/projects/{project}/ckan/status"],
  jc_datasource_discover: ["POST", "/projects/{project}/datasource-discovery", ["kind", "url"]],
  // A JSON import of manifests (T-7143); its `dryRun` is the REST door's `?dryRun=All`.
  jc_project_import: ["POST", "/projects/{project}/import", ["manifests", "conflictPolicy", "orgDomain", "spaceMapping"]],

  // The Portal doors of a space's person records, its drift and a person's alerts (T-7105).
  portal_space_quality: ["GET", "/projects/{project}/spaces/{space}/quality"],
  portal_space_quality_checks: ["GET", "/projects/{project}/spaces/{space}/quality/checks"],
  portal_space_temporal_audit: ["GET", "/projects/{project}/spaces/{space}/temporal-audit"],
  portal_space_retention_deleted: ["GET", "/projects/{project}/spaces/{space}/retention/deleted"],
  portal_space_retention_preview: ["POST", "/projects/{project}/spaces/{space}/retention/preview"],
  portal_space_subject_export: ["POST", "/projects/{project}/spaces/{space}/subjects/export"],
  portal_erasure_list: ["GET", "/projects/{project}/spaces/{space}/erasures"],
  portal_erasure_file: ["POST", "/projects/{project}/spaces/{space}/erasures"],
  portal_erasure_get: ["GET", "/projects/{project}/spaces/{space}/erasures/{id}"],
  portal_erasure_approve: ["POST", "/projects/{project}/spaces/{space}/erasures/{id}/approve"],
  portal_erasure_decline: ["POST", "/projects/{project}/spaces/{space}/erasures/{id}/decline"],
  portal_comment_list: ["GET", "/projects/{project}/spaces/{space}/comments"],
  portal_comment_add: ["POST", "/projects/{project}/spaces/{space}/comments"],
  portal_comment_remove: ["DELETE", "/projects/{project}/spaces/{space}/comments/{id}"],
  portal_trash_list: ["GET", "/projects/{project}/spaces/{space}/trash"],
  portal_trash_keep: ["POST", "/projects/{project}/spaces/{space}/trash"],
  portal_trash_forget: ["DELETE", "/projects/{project}/spaces/{space}/trash/{id}"],
  portal_drift_list: ["GET", "/projects/{project}/drift"],
  portal_drift_revert: ["POST", "/projects/{project}/drift/{space}/{id}/revert"],
  portal_drift_adopt: ["POST", "/projects/{project}/drift/{space}/{id}/adopt"],
  portal_alert_list: ["GET", "/alerts"],
  portal_alert_subscribe: ["PUT", "/alerts"],
  portal_alert_unsubscribe: ["DELETE", "/alerts/{id}"],
  portal_alert_mute: ["POST", "/alerts/{id}/mute"],
  portal_alert_notice_list: ["GET", "/alerts/notices"],
  portal_alert_notice_read: ["POST", "/alerts/notices/{id}/read"],

  // The Portal doors of a pipeline's runs, rejected records, source deletes, source check and
  // access requests (T-7109).
  portal_pipeline_run_list: ["GET", "/projects/{project}/pipelines/{pipeline}/runs"],
  portal_pipeline_run_start: ["POST", "/projects/{project}/pipelines/{pipeline}/runs"],
  portal_pipeline_run_log: ["GET", "/projects/{project}/pipelines/{pipeline}/runs/{run}/log"],
  portal_pipeline_latest_runs: ["GET", "/projects/{project}/pipeline-runs"],
  portal_pipeline_health: ["GET", "/projects/{project}/pipeline-health"],
  portal_pipeline_metrics: ["GET", "/projects/{project}/pipelines/{pipeline}/metrics"],
  portal_pipeline_samples: ["GET", "/projects/{project}/pipelines/{pipeline}/samples"],
  portal_pipeline_stats: ["GET", "/projects/{project}/pipelines/{pipeline}/stats"],
  portal_pipeline_rejected: ["GET", "/projects/{project}/pipelines/{pipeline}/rejected"],
  portal_pipeline_rejected_groups: ["GET", "/projects/{project}/pipelines/{pipeline}/rejected/groups"],
  portal_pipeline_rejected_retry: ["POST", "/projects/{project}/pipelines/{pipeline}/rejected/retry"],
  portal_pipeline_source_deletes: ["GET", "/projects/{project}/pipelines/{pipeline}/source-deletes"],
  portal_pipeline_source_decide: ["POST", "/projects/{project}/pipelines/{pipeline}/source-deletes"],
  portal_space_source_removals: ["GET", "/projects/{project}/spaces/{space}/source-removals"],
  portal_pipeline_source_check: ["POST", "/projects/{project}/pipelines/{pipeline}/source-check"],
  portal_pipeline_access_request_list: ["GET", "/projects/{project}/pipelines/{pipeline}/access-requests"],
  portal_pipeline_access_request_file: ["POST", "/projects/{project}/pipelines/{pipeline}/access-requests"],
  portal_pipeline_access_request_endpoints: [
    "GET",
    "/projects/{project}/pipelines/{pipeline}/access-requests/endpoints",
  ],
  portal_access_request_incoming: ["GET", "/projects/{project}/access-requests"],
  // The pipeline workbench's mapping file, templates, flow map and a type's attributes (T-7120).
  portal_pipeline_bento: ["GET", "/projects/{project}/pipelines/{pipeline}/bento"],
  portal_pipeline_template_list: ["GET", "/projects/{project}/pipelinetemplates"],
  portal_pipeline_template_create: ["POST", "/projects/{project}/pipelinetemplates"],
  portal_pipeline_template_delete: ["DELETE", "/projects/{project}/pipelinetemplates/{id}"],
  portal_flow_map: ["GET", "/projects/{project}/flow-map"],
  portal_type_attributes: ["GET", "/projects/{project}/spaces/{space}/types/{type}/attributes"],
  // The Apps pages' build links, builds, audience, dashboard editor, checks and templates (T-7121).
  portal_app_build: ["GET", "/projects/{project}/apps/{name}/build"],
  portal_app_rebuild: ["POST", "/projects/{project}/apps/{name}/rebuild"],
  portal_app_builds: ["GET", "/projects/{project}/apps/{name}/builds"],
  portal_app_restore: ["POST", "/projects/{project}/apps/{name}/restore"],
  portal_app_audience: ["GET", "/projects/{project}/apps/{name}/audience"],
  portal_app_dashboard: ["GET", "/projects/{project}/apps/{name}/dashboard"],
  portal_app_dashboard_propose: ["POST", "/projects/{project}/apps/{name}/dashboard"],
  portal_app_checks: ["GET", "/projects/{project}/app-checks"],
  portal_app_rename_shapes: ["POST", "/projects/{project}/apps/rename-shapes"],
  portal_app_from_template: ["POST", "/projects/{project}/app-templates/{name}/apps"],
  // The Release tab's registry entry and its repoint (T-7166).
  portal_project_registry: ["GET", "/projects/{project}/registry"],
  portal_project_repoint: ["PUT", "/projects/{project}/registry"],
  // The people reads and edits and the Organization reads of any page (T-7167).
  portal_people_list: ["GET", "/organization/people"],
  portal_person_create: ["POST", "/organization/people"],
  portal_person_get: ["GET", "/organization/people/{person}"],
  portal_person_edit: ["PATCH", "/organization/people/{person}"],
  portal_portal_host_list: ["GET", "/organization/portal-hosts"],
  portal_widget_list: ["GET", "/organization/widgets"],
  portal_organization_model_list: ["GET", "/organization/datamodels"],
  portal_health_check_list: ["GET", "/projects/{project}/health-checks"],
  portal_project_get: ["GET", "/projects/{project}"],
  portal_project_delete: ["DELETE", "/projects/{project}"],
  // A space's and an Endpoint's own readings and the project's activity feed (T-7172).
  portal_space_surface_fit: ["GET", "/projects/{project}/spaces/{space}/surface-fit"],
  portal_space_usage_recalculate: ["POST", "/projects/{project}/spaces/{space}/usage/recalculate"],
  portal_space_view_list: ["GET", "/projects/{project}/spaces/{space}/views"],
  portal_space_view_create: ["POST", "/projects/{project}/spaces/{space}/views"],
  portal_space_view_replace: ["PUT", "/projects/{project}/spaces/{space}/views/{id}"],
  portal_space_view_delete: ["DELETE", "/projects/{project}/spaces/{space}/views/{id}"],
  portal_endpoint_admission: ["GET", "/projects/{project}/endpoints/{endpoint}/admission"],
  portal_endpoint_access_matrix: ["GET", "/projects/{project}/endpoints/{endpoint}/access/matrix"],
  portal_endpoint_access_attribute: ["GET", "/projects/{project}/endpoints/{endpoint}/access/attributes"],
  portal_endpoint_usage: ["GET", "/projects/{project}/endpoints/{endpoint}/usage"],
  portal_project_usage: ["GET", "/projects/{project}/usage/endpoints"],
  portal_activity_list: ["GET", "/projects/{project}/activity"],
  // A person's own connectors and paired tab, the model tools, the kinds' schemas and forms, the
  // Apps shared with the person, their feedback and a project's assistant (T-7173).
  portal_own_connector_list: ["GET", "/connectors"],
  portal_own_connector_options: ["GET", "/connectors/options"],
  portal_own_connector_create: ["POST", "/connectors"],
  portal_own_connector_edit: ["PATCH", "/connectors/{id}"],
  portal_own_connector_revoke: ["DELETE", "/connectors/{id}"],
  portal_own_connector_activity: ["GET", "/connectors/{id}/activity"],
  portal_connector_tab_pair: ["POST", "/connectors/{id}/tab"],
  portal_connector_tab_unpair: ["DELETE", "/connectors/{id}/tab"],
  portal_model_catalogue: ["GET", "/tools/sdm-catalog"],
  portal_model_compile: ["POST", "/tools/generate"],
  portal_model_import: ["POST", "/tools/import-sdm"],
  portal_kind_schema: ["GET", "/kinds/{kind}/schema"],
  portal_form_list: ["GET", "/forms"],
  portal_shared_app_list: ["GET", "/shared-apps"],
  portal_feedback_send: ["POST", "/feedback"],
  portal_assistant_access: ["GET", "/projects/{project}/assistant/access"],
  portal_conversation_start: ["POST", "/projects/{project}/assistant/conversations"],
  portal_endpoint_render: ["POST", "/projects/{project}/assistant/propose-endpoint"],
  portal_app_templates: ["GET", "/app-templates"],
  portal_blueprint_list: ["GET", "/blueprints"],
  portal_access_request_approve: ["POST", "/projects/{project}/access-requests/{id}/approve"],
  portal_access_request_decline: ["POST", "/projects/{project}/access-requests/{id}/decline"],
  portal_access_request_revoke: ["POST", "/projects/{project}/access-requests/{id}/revoke"],
  // The knowledge assistant's doors: its sources, deployments and stored conversations (T-7111).
  portal_knowledge_source_list: ["GET", "/projects/{project}/knowledge/sources"],
  portal_knowledge_page_list: ["GET", "/projects/{project}/knowledge/sources/{source}/pages"],
  portal_knowledge_page_links: ["GET", "/projects/{project}/knowledge/sources/{source}/pages/{page}/links"],
  portal_knowledge_document_list: ["GET", "/projects/{project}/knowledge/sources/{source}/documents"],
  portal_knowledge_search: ["GET", "/projects/{project}/knowledge/sources/{source}/search"],
  portal_knowledge_passage_list: ["GET", "/projects/{project}/knowledge/sources/{source}/passages"],
  portal_knowledge_error_list: ["GET", "/projects/{project}/knowledge/sources/{source}/errors"],
  portal_knowledge_inclusion_set: ["POST", "/projects/{project}/knowledge/sources/{source}/inclusion"],
  portal_knowledge_recrawl: ["POST", "/projects/{project}/knowledge/sources/{source}/recrawl"],
  portal_knowledge_file_list: ["GET", "/projects/{project}/knowledge/sources/{source}/files"],
  portal_knowledge_file_remove: ["DELETE", "/projects/{project}/knowledge/sources/{source}/files/{id}"],
  portal_knowledge_context_preview: ["POST", "/projects/{project}/knowledge/deployments/context"],
  portal_knowledge_usage: ["GET", "/projects/{project}/knowledge/deployments/{deployment}/usage"],
  portal_knowledge_answers_forget: [
    "DELETE",
    "/projects/{project}/knowledge/deployments/{deployment}/answers",
  ],
  portal_knowledge_conversation_list: ["GET", "/projects/{project}/knowledge/conversations"],
  portal_knowledge_conversation_usage: ["GET", "/projects/{project}/knowledge/conversations/usage"],
  portal_knowledge_conversation_get: ["GET", "/projects/{project}/knowledge/conversations/{conversation}"],
  portal_knowledge_conversation_erase: [
    "DELETE",
    "/projects/{project}/knowledge/conversations/{conversation}",
  ],
  portal_knowledge_conversation_same: [
    "GET",
    "/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same",
  ],
  portal_mqtt_allowance: ["GET", "/projects/{project}/mqtt/allowance"],
  portal_mqtt_broker_access: ["GET", "/projects/{project}/mqttbrokers/{broker}/access"],
  portal_mqtt_device_options: ["GET", "/projects/{project}/mqttbrokers/{broker}/devices"],
  portal_mqtt_device_propose: ["POST", "/projects/{project}/mqttbrokers/{broker}/devices"],
  portal_mqtt_test_publish: ["POST", "/projects/{project}/mqttbrokers/{broker}/test-publish"],
  portal_mqtt_watch: ["GET", "/projects/{project}/mqttbrokers/{broker}/watch"],
  portal_mqtt_measured: ["GET", "/projects/{project}/mqttbrokers/{broker}/measured"],
  portal_mqtt_captured_list: ["GET", "/projects/{project}/mqttbrokers/{broker}/captured"],
  portal_mqtt_captured_clear: ["DELETE", "/projects/{project}/mqttbrokers/{broker}/captured"],
  portal_mqtt_binding_list: ["GET", "/projects/{project}/mqttbrokers/{broker}/bindings"],
  portal_grafana_list: ["GET", "/projects/{project}/grafana"],
  portal_grafana_options: ["GET", "/projects/{project}/grafana/options"],
  portal_grafana_save: ["POST", "/projects/{project}/grafana"],
  portal_grafana_remove: ["DELETE", "/projects/{project}/grafana/{id}"],
  portal_grafana_dashboards: ["GET", "/projects/{project}/grafana/dashboards"],
  portal_grafana_dashboard: ["GET", "/projects/{project}/grafana/dashboards/{uid}"],
  portal_grafana_panel: ["GET", "/projects/{project}/grafana/dashboards/{uid}/panels/{panel}"],
  portal_grafana_variable: ["GET", "/projects/{project}/grafana/dashboards/{uid}/variables/{name}"],
  portal_grafana_builder: ["GET", "/projects/{project}/grafana/builder"],
  portal_grafana_builder_create: ["POST", "/projects/{project}/grafana/builder"],
  portal_grafana_builder_preview: ["POST", "/projects/{project}/grafana/builder/preview"],
  portal_grafana_builder_get: ["GET", "/projects/{project}/grafana/builder/{uid}"],
  portal_grafana_builder_update: ["PUT", "/projects/{project}/grafana/builder/{uid}"],
  portal_grafana_builder_versions: ["GET", "/projects/{project}/grafana/builder/{uid}/versions"],
  portal_grafana_builder_version: ["GET", "/projects/{project}/grafana/builder/{uid}/versions/{sha}"],
  portal_grafana_builder_restore: ["POST", "/projects/{project}/grafana/builder/{uid}/versions/{sha}/restore"],
  portal_kpi_check: ["POST", "/projects/{project}/kpis/check"],
  // The publish flow's draft (T-7117): the registry read whose handler is the REST door's.
  jc_catalogue_draft: ["POST", "/projects/{project}/catalogue/drafts", ["endpoint"]],
  // A DataSource's connection test, webhook address, test event and paging guess (T-7117).
  portal_datasource_test: ["POST", "/projects/{project}/datasources/{name}/test"],
  portal_datasource_webhook: ["GET", "/projects/{project}/datasources/{name}/webhook"],
  portal_datasource_test_event: ["POST", "/projects/{project}/datasources/{name}/test-event"],
  portal_datasource_paging: ["POST", "/projects/{project}/datasource-paging"],
  // An open data portal's harvest sources and datasets (T-7117), bound to the Organization.
  portal_portal_source_test: ["POST", "/projects/org/portals/{name}/sources/{source}/test"],
  portal_portal_source_run: ["POST", "/projects/org/portals/{name}/sources/{source}/runs"],
  portal_portal_source_runs: ["GET", "/projects/org/portals/{name}/sources/{source}/runs"],
  portal_portal_datasets: ["GET", "/projects/org/portals/{name}/datasets"],
  // A project's copies (T-7116): the registry's workspace operations, run by the REST doors' handlers.
  jc_workspace_open: ["POST", "/projects/{project}/workspaces", ["name", "title", "ttlDays", "scope"]],
  jc_workspace_compare: ["GET", "/projects/{project}/workspaces/{name}/compare"],
  jc_workspace_update_from_main: ["POST", "/projects/{project}/workspaces/{name}/update", ["resolutions"]],
  jc_workspace_propose: ["POST", "/projects/{project}/workspaces/{name}/propose"],
  jc_workspace_discard: ["DELETE", "/projects/{project}/workspaces/{name}"],
  jc_workspace_preview_get: ["GET", "/projects/{project}/workspaces/{name}/preview"],
  jc_workspace_preview_start: ["POST", "/projects/{project}/workspaces/{name}/preview"],
  jc_workspace_preview_stop: ["DELETE", "/projects/{project}/workspaces/{name}/preview"],
  // The shared drafts and a run's turns (T-7119): the drafts' REST doors run the registry's
  // operations; a run's answer and message are Portal doors, its cancel and publish operations.
  jc_draft_list: ["GET", "/projects/{project}/drafts"],
  jc_draft_get: ["GET", "/projects/{project}/drafts/{kind}/{name}"],
  jc_draft_put: ["PUT", "/projects/{project}/drafts/{kind}/{name}", ["manifest", "expectedVersion"]],
  jc_draft_drop: ["DELETE", "/projects/{project}/drafts/{kind}/{name}"],
  jc_run_create: [
    "POST",
    "/projects/{project}/agent-runs",
    ["appName", "appClass", "endpointName", "endpointNames", "prompt", "visibility", "dataNeeds", "kind", "profile", "unattended"],
  ],
  jc_run_cancel: ["POST", "/projects/{project}/agent-runs/{id}/cancel"],
  jc_run_publish: ["POST", "/projects/{project}/agent-runs/{id}/publish"],
  portal_run_answer: ["POST", "/projects/{project}/agent-runs/{id}/answers"],
  portal_run_message: ["POST", "/projects/{project}/agent-runs/{id}/messages"],
  // The Change pages' doors the files stubbing the REST doors reach (T-7126).
  portal_change_outside: ["GET", "/projects/{project}/changes/outside"],
  portal_change_manifests: ["GET", "/projects/{project}/changes/{id}/manifests"],
  portal_change_restore: ["POST", "/projects/{project}/changes/{id}/restore"],
  portal_change_undo: ["POST", "/projects/{project}/changes/{id}/undo"],
  portal_organization_change_restore: ["POST", "/projects/org/changes/{id}/restore"],
  portal_organization_change_undo: ["POST", "/projects/org/changes/{id}/undo"],
  // An MCP call's question and a broker MCP write's, decided on their pages (T-7132).
  portal_mcp_elicitation_get: ["GET", "/projects/{projectName}/mcp-elicitations/{id}"],
  portal_mcp_elicitation_decide: ["POST", "/projects/{projectName}/mcp-elicitations/{id}/decision"],
  portal_mcp_confirmation_get: ["GET", "/mcp-confirmations/{id}"],
  portal_mcp_confirmation_decide: ["POST", "/mcp-confirmations/{id}/decision"],
};

/**
 * An operation whose answer is not its REST door's, and the service's answer of the door's: a
 * workspace brought back answers the Change as every proposing operation does (api/mutate.rs
 * `ProposeOutcome::Change`).
 */
function proposed(body: unknown): unknown {
  const change = body as { metadata?: { name?: string }; status?: { lane?: string; mergeRequest?: string } } | null;
  return { changeId: change?.metadata?.name, lane: change?.status?.lane, url: change?.status?.mergeRequest, change };
}

const ANSWERS: Record<string, (body: unknown) => unknown> = {
  jc_workspace_propose: proposed,
  jc_run_publish: proposed,
};

/** The answer {@link ANSWERS} makes of the REST door's for the operation `request` calls, if any. */
export function answerShape(request: Request): ((body: unknown) => unknown) | undefined {
  const name = DOOR.exec(decodeURIComponent(new URL(request.url).pathname))?.[2];
  return name === undefined ? undefined : ANSWERS[name];
}

/** Whether `request` calls one of {@link OPERATIONS}. */
export function callsOperation(request: Request): boolean {
  const name = DOOR.exec(decodeURIComponent(new URL(request.url).pathname))?.[2];
  return name !== undefined && Object.hasOwn(OPERATIONS, name);
}

const DOORS: Record<string, Door> = {
  ...OPERATIONS,
  // The Organization-bound people actions whose registry operation is their REST route
  // (route_coverage_tests names each), answered the same way (T-7101).
  jc_person_disable: ["POST", "/organization/people/{id}/disable"],
  jc_person_enable: ["POST", "/organization/people/{id}/enable"],
  jc_person_sign_out: ["POST", "/organization/people/{id}/sign-out"],
  portal_people_remove: ["POST", "/organization/people/remove"],
  portal_people_name_by_id: ["POST", "/organization/people/name-by-id"],
  portal_person_names: ["GET", "/organization/people/names"],
  portal_person_reset_password: [
    "POST",
    "/organization/people/{person}/reset-password",
  ],
  portal_person_resend_invitation: [
    "POST",
    "/organization/people/{person}/resend-invitation",
  ],
  portal_person_remove_second_factor: [
    "POST",
    "/organization/people/{person}/remove-second-factor",
  ],
  portal_person_delete: ["DELETE", "/organization/people/{person}"],
  portal_person_consent_revoke: [
    "DELETE",
    "/organization/people/{person}/consents/{client}",
  ],
  portal_connector_list: ["GET", "/organization/connectors"],
  portal_connector_revoke: ["DELETE", "/organization/connectors/{connector}"],
  portal_notification_list: ["GET", "/notifications"],
  portal_notification_read: ["POST", "/notifications/{notification}/read"],
  portal_preferences_get: ["GET", "/preferences"],
  portal_preferences_set: ["PUT", "/preferences"],
  portal_preferences_recent: ["POST", "/preferences/recent"],
  portal_visualization_template: ["GET", "/visualization-templates/{template}"],
  portal_secret_list: ["GET", "/projects/{project}/secrets"],
  portal_secret_set: ["POST", "/projects/{project}/secrets"],
  portal_serviceaccount_secret_issue: [
    "POST",
    "/projects/{project}/serviceaccounts/{account}/secret",
  ],
  portal_serviceaccount_revoke: [
    "POST",
    "/projects/{project}/serviceaccounts/{account}/revoke",
  ],
  portal_access_request_list: [
    "GET",
    "/projects/{project}/endpoints/{endpoint}/access-requests",
  ],
  portal_access_request_file: [
    "POST",
    "/projects/{project}/endpoints/{endpoint}/access-requests",
  ],
  portal_endpoint_quality: [
    "GET",
    "/projects/{project}/endpoints/{endpoint}/quality",
  ],
  portal_opendata_pages: ["GET", "/organization/opendata/pages"],
  portal_opendata_page_preview: ["POST", "/organization/opendata/pages/preview"],
  portal_opendata_page_set: ["PUT", "/organization/opendata/pages/{page}/{lang}"],
  portal_opendata_page_revert: [
    "DELETE",
    "/organization/opendata/pages/{page}/{lang}",
  ],
  portal_organization_change_list: ["GET", "/projects/org/changes"],
  portal_organization_change_history: ["GET", "/projects/org/changes/history"],
  portal_organization_change_get: ["GET", "/projects/org/changes/{id}"],
  // The `org` namespace's own Changes decided (T-7122): `body` is the REST door's whole body.
  portal_organization_change_approve: ["POST", "/projects/org/changes/{id}/approve"],
  portal_organization_change_reject: ["POST", "/projects/org/changes/{id}/reject"],
  portal_organization_change_manifests: [
    "GET",
    "/projects/org/changes/{id}/manifests",
  ],
};

const DOOR =
  /^\/api\/v1\/(?:Organization|projects\('((?:[^']|'')*)'\))\/JoinedContext\.Portal\.(\w+)(?:\(([^)]*)\))?$/;

/**
 * The REST request whose handler answers the Portal door `request` calls, or nothing for any other
 * request: its path members filled, its other members the query, an action's `body` the JSON body.
 * The server proves the two answer alike (odata_org_tests `the_portal_doors_answer_as_their_rest_doors`),
 * so a page test keeps its REST stub and wraps the stub's answer with `asOData`.
 */
export async function doorAsRest(
  request: Request,
): Promise<Request | undefined> {
  const at = new URL(request.url);
  const found = DOOR.exec(decodeURIComponent(at.pathname));
  const door = found ? DOORS[found[2]] : undefined;
  if (!found || !door) return undefined;
  const params: Record<string, unknown> = {};
  for (const argument of (found[3] ?? "").split(",").filter(Boolean)) {
    const [name, alias] = argument.split("=");
    const value = at.searchParams.get(alias);
    if (value !== null) params[name] = literal(value);
  }
  if (request.method === "POST")
    Object.assign(
      params,
      (await request
        .clone()
        .json()
        .catch(() => ({}))) as object,
    );
  const project = (found[1] ?? "").replaceAll("''", "'");
  let path = door[1].replace("{project}", encodeURIComponent(project));
  const rest = new URL("/api/v1", at.origin);
  let body: string | undefined;
  const members: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(params)) {
    if (path.includes(`{${name}}`))
      path = path.replace(`{${name}}`, encodeURIComponent(String(value)));
    else if (name === "body") body = JSON.stringify(value);
    else if (name === "vars")
      // A dashboard's variables, each value one `var-{name}` member (odata_portal.rs Place::Vars).
      for (const [variable, picked] of Object.entries(value as Record<string, string | string[]>))
        for (const each of [picked].flat()) rest.searchParams.append(`var-${variable}`, each);
    else if (door[2]?.includes(name)) members[name] = value;
    else if (name === "dryRun" && found[2] === "jc_project_import") {
      if (value === true) rest.searchParams.set(name, "All");
    } else rest.searchParams.set(name, String(value));
  }
  if (Object.keys(members).length) body = JSON.stringify(members);
  rest.pathname = `/api/v1${path}`;
  // The service sends the REST handler the call's own headers (the session, the CSRF token),
  // never its OData content headers (odata_org.rs `portal_door`).
  const headers = new Headers(request.headers);
  for (const name of ["content-type", "content-length", "accept", "if-match"]) headers.delete(name);
  for (const name of [...headers.keys()]) if (name.startsWith("odata-")) headers.delete(name);
  if (body) headers.set("Content-Type", "application/json");
  return new Request(rest, { method: door[0], body, headers });
}

const CHANGE_KEY = /^\/api\/v1\/changes\(project='((?:[^']|'')*)',name='((?:[^']|'')*)'\)$/;
const DECIDE =
  /^\/api\/v1\/changes\(project='((?:[^']|'')*)',name='((?:[^']|'')*)'\)\/JoinedContext\.Portal\.(approve|reject)$/;

/** Whether `url` calls an action bound to an entity: a `propose_*`, or a Change's `approve` or `reject`. */
export function actsOnEntity(url: string): boolean {
  return url.includes("JoinedContext.Portal.propose_") || DECIDE.test(decodeURIComponent(new URL(url, "http://localhost").pathname));
}

const CHANGES = /^\/api\/v1\/projects\('((?:[^']|'')*)'\)\/changes$/;
const UNDER = /^\/api\/v1\/projects\('((?:[^']|'')*)'\)\/(\w+)$/;
const ORG_SET = /^\/api\/v1\/(\w+)$/;
const WORKSPACES =
  /^\/api\/v1\/(?:projects\('((?:[^']|'')*)'\)\/workspaces|workspaces\(project='((?:[^']|'')*)',name='((?:[^']|'')*)'\))$/;
const RUNS =
  /^\/api\/v1\/(?:projects\('((?:[^']|'')*)'\)\/runs|runs\(project='((?:[^']|'')*)',id='((?:[^']|'')*)'\))$/;
/** The REST list's query parameter of each member a runs `$filter` narrows by (api/odata.ts listRuns). */
const RUN_FILTERS: Record<string, string> = { appName: "app", kind: "kind", createdBy: "mine", origin: "origin" };
const KEYED =
  /^\/api\/v1\/(\w+)\((?:project='((?:[^']|'')*)',name='((?:[^']|'')*)'|'((?:[^']|'')*)')\)$/;

/**
 * The REST read whose handler answers a manifest read of the service (api/odata.ts
 * `listManifests`, `readManifest`: a set with `$select` of `spec`), and whether it reads one
 * manifest; nothing for any other request. The server proves the two answer alike (odata_org_tests
 * `a_manifests_spec_status_and_metadata_answer_as_the_rest_door`).
 */
export function manifestAsRest(
  url: string | URL,
): { rest: URL; one: boolean; organization?: boolean; row?: boolean } | undefined {
  const at = new URL(String(url), "http://localhost");
  const path = decodeURIComponent(at.pathname);
  const unquote = (text: string) => text.replaceAll("''", "'");
  // A project's copies (api/odata.ts listWorkspaces, readWorkspace): the REST rows are the
  // service's (workspace_api_tests `the_workspaces_set_answers_scope_and_changes_as_the_rest_doors`).
  // A Change by its key (api/odata.ts readChange): its row with `$select` of every member is the
  // REST door's answer (odata_org_tests `a_changes_detail_answers_as_the_rest_door`, T-7122).
  const change = CHANGE_KEY.exec(path);
  if (change) {
    const [project, id] = [change[1], change[2]].map((text) => encodeURIComponent(unquote(text)));
    return { rest: new URL(`/api/v1/projects/${project}/changes/${id}`, at.origin), one: true, row: true };
  }
  // A project's open Changes (api/odata.ts listChanges): the rows with `$select` of the list's
  // members are the REST list's items (odata_org_tests `the_change_list_answers_its_detail_from_the_list`).
  const listed = CHANGES.exec(path);
  if (listed)
    return { rest: new URL(`/api/v1/projects/${encodeURIComponent(unquote(listed[1]))}/changes`, at.origin), one: false, row: true };
  const copies = WORKSPACES.exec(path);
  if (copies) {
    const project = encodeURIComponent(unquote(copies[1] ?? copies[2]));
    const name = copies[3] === undefined ? "" : `/${encodeURIComponent(unquote(copies[3]))}`;
    return { rest: new URL(`/api/v1/projects/${project}/workspaces${name}`, at.origin), one: name !== "", row: true };
  }
  // A project's runs and one run (api/odata.ts listRuns, readRun): the rows with `$select` of the
  // run's members are the REST door's answer (odata_org_tests `keyed_project_reads_are_sets_not_functions`).
  const runs = RUNS.exec(path);
  if (runs) {
    const project = encodeURIComponent(unquote(runs[1] ?? runs[2]));
    if (runs[3] !== undefined)
      return { rest: new URL(`/api/v1/projects/${project}/agent-runs/${encodeURIComponent(unquote(runs[3]))}`, at.origin), one: true, row: true };
    const rest = new URL(`/api/v1/projects/${project}/agent-runs`, at.origin);
    // The set holds every origin (T-7145): no `origin` in `$filter` is the REST list's `origin=all`,
    // `origin eq 'person'` its default.
    rest.searchParams.set("origin", "all");
    for (const [, member, value] of (at.searchParams.get("$filter") ?? "").matchAll(/(\w+) eq '((?:[^']|'')*)'/g)) {
      const name = RUN_FILTERS[member];
      if (name === "origin" && unquote(value) === "person") rest.searchParams.delete("origin");
      else if (name) rest.searchParams.set(name, name === "mine" ? "true" : unquote(value));
    }
    const top = at.searchParams.get("$top");
    if (top !== null) rest.searchParams.set("limit", top);
    return { rest, one: false, row: true };
  }
  if (!(at.searchParams.get("$select") ?? "").split(",").includes("spec"))
    return undefined;
  const rest = (project: string, set: string, name?: string): URL => {
    // The `org` namespace's own Roles are the Organization set `organizationRoles`.
    const plural = set === "organizationRoles" ? "roles" : set;
    const tail = name === undefined ? "" : `/${encodeURIComponent(name)}`;
    return new URL(
      `/api/v1/projects/${encodeURIComponent(project)}/${plural}${tail}`,
      at.origin,
    );
  };
  const under = UNDER.exec(path);
  if (under) return { rest: rest(unquote(under[1]), under[2]), one: false };
  const keyed = KEYED.exec(path);
  if (keyed)
    return keyed[4] === undefined
      ? {
          rest: rest(unquote(keyed[2]), keyed[1], unquote(keyed[3])),
          one: true,
        }
      : { rest: rest("org", keyed[1], unquote(keyed[4])), one: true };
  // The `Organization` singleton: the Organization of the REST list the service serves (T-7104).
  if (path === "/api/v1/Organization")
    return { rest: rest("org", "organizations"), one: true, organization: true };
  const org = ORG_SET.exec(path);
  return org ? { rest: rest("org", org[1]), one: false } : undefined;
}

/** A REST manifest as the service's row of it with `$select` of the manifest's members. */
function manifestRow(
  manifest: Record<string, unknown>,
): Record<string, unknown> {
  const metadata = (manifest.metadata ?? {}) as { name?: string };
  return {
    "@odata.etag": `W/"${metadata.name ?? ""}"`,
    name: metadata.name,
    kind: manifest.kind,
    apiVersion: manifest.apiVersion ?? null,
    metadata: manifest.metadata,
    spec: manifest.spec ?? null,
    status: manifest.status ?? null,
  };
}

/** The REST read's answer as the service gives the manifest read's. */
export async function manifestAnswer(
  rest: Response,
  one: boolean,
  organization = false,
  row = false,
): Promise<Response> {
  if (!rest.ok) return asOData(rest);
  const body = (await rest.json()) as Record<string, unknown>;
  // A REST row that is the service's row as it is: a list's `items` are the set's `value`.
  if (row)
    return new Response(JSON.stringify(one ? body : { value: body.items ?? [] }), {
      status: 200,
      headers: { "Content-Type": "application/json", "OData-Version": "4.01" },
    });
  const items = (body.items ?? []) as Record<string, unknown>[];
  // A list of several is the installation's address, which serves no one Organization's service.
  if (organization && items.length > 1)
    return odataError(404, "several-organizations", "this address serves several Organizations; read one at its own host");
  const answer = organization
    ? items[0]
      ? manifestRow(items[0])
      : { name: "org", metadata: null, spec: null, status: null }
    : one
    ? manifestRow(body)
    : {
        value: ((body.items ?? []) as Record<string, unknown>[]).map(
          manifestRow,
        ),
      };
  return new Response(JSON.stringify(answer), {
    status: 200,
    headers: { "Content-Type": "application/json", "OData-Version": "4.01" },
  });
}

/**
 * Routes each Portal door call of the `fetch` stubbed now to its REST door's stub, its answer
 * wrapped as the service wraps it: a page test that stubs `fetch` itself calls this after.
 */
export function doorsAsRest(): void {
  const inner = globalThis.fetch;
  vi.stubGlobal(
    "fetch",
    async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const request =
        input instanceof Request
          ? input
          : new Request(new URL(String(input), window.location.origin), init);
      const door = await doorAsRest(request);
      return door ? asOData(await inner(door)) : inner(input, init);
    },
  );
}

type Container = Record<string, { $Collection?: boolean; $Type?: string }>;
const SETS = (metadata as unknown as Record<string, { Service: Container }>)[
  "JoinedContext.Portal"
].Service;

/** The REST plural of `kind`: its set in `$metadata` (the `org` namespace's Roles are `roles`). */
function pluralOfKind(kind: unknown): string | undefined {
  const set = Object.entries(SETS).find(
    ([, s]) => s.$Collection && s.$Type === `JoinedContext.Portal.${String(kind)}`,
  )?.[0];
  return set === "organizationRoles" ? "roles" : set;
}

const PROPOSE =
  /^\/api\/v1\/(?:projects\('((?:[^']|'')*)'\)|(\w+)\((?:project='((?:[^']|'')*)',name='((?:[^']|'')*)'|'((?:[^']|'')*)')\))\/JoinedContext\.Portal\.(propose_create|propose_update|propose_delete)$/;

/**
 * The REST write whose handler answers a `propose_*` action (api/odata.ts `proposeManifest`,
 * `proposeDelete`): `POST`, `PUT`, `PATCH` or `DELETE` of the manifest door with `?dryRun=All` for
 * a check and the action's `If-Match`, or nothing for any other request. The service runs that
 * door and answers its response as it is (odata_org.rs `act_inner`).
 */
export async function proposalAsRest(request: Request): Promise<Request | undefined> {
  if (request.method !== "POST") return undefined;
  const at = new URL(request.url);
  // A Change's `approve` or `reject` (api/odata.ts approveChange, rejectChange): the REST door's
  // POST with the action's parameters as its body (odata_org.rs `act_inner`).
  const decided = DECIDE.exec(decodeURIComponent(at.pathname));
  if (decided) {
    const [project, id] = [decided[1], decided[2]].map((text) => encodeURIComponent(text.replaceAll("''", "'")));
    const headers = new Headers(request.headers);
    headers.delete("OData-Version");
    headers.set("Content-Type", "application/json");
    return new Request(new URL(`/api/v1/projects/${project}/changes/${id}/${decided[3]}`, at.origin), {
      method: "POST",
      headers,
      body: await request.clone().text(),
    });
  }
  const found = PROPOSE.exec(decodeURIComponent(at.pathname));
  if (!found) return undefined;
  const unquote = (text: string) => text.replaceAll("''", "'");
  const params = (await request
    .clone()
    .json()
    .catch(() => ({}))) as { manifest?: { kind?: unknown }; patch?: unknown; dryRun?: boolean };
  const action = found[6];
  const [project, plural, name] =
    found[1] !== undefined
      ? [unquote(found[1]), pluralOfKind(params.manifest?.kind), undefined]
      : found[3] !== undefined
        ? [unquote(found[3]), pluralOfKind(SETS[found[2]]?.$Type?.split(".").pop()), unquote(found[4])]
        : ["org", pluralOfKind(SETS[found[2]]?.$Type?.split(".").pop()), unquote(found[5])];
  const tail = name === undefined ? "" : `/${encodeURIComponent(name)}`;
  const rest = new URL(
    `/api/v1/projects/${encodeURIComponent(project)}/${plural ?? "unknown"}${tail}`,
    at.origin,
  );
  if (params.dryRun) rest.searchParams.set("dryRun", "All");
  if (typeof (params as { confirm?: unknown }).confirm === "string")
    rest.searchParams.set("confirm", (params as { confirm: string }).confirm);
  // The session's headers as sent (the CSRF token, If-Match); the service's own are not the door's.
  const headers = new Headers(request.headers);
  headers.delete("OData-Version");
  headers.delete("Content-Type");
  if (action === "propose_delete") return new Request(rest, { method: "DELETE", headers });
  const patch = action === "propose_update" && params.patch !== undefined;
  headers.set("Content-Type", patch ? "application/merge-patch+json" : "application/json");
  return new Request(rest, {
    method: action === "propose_create" ? "POST" : patch ? "PATCH" : "PUT",
    headers,
    body: JSON.stringify(patch ? params.patch : params.manifest),
  });
}
