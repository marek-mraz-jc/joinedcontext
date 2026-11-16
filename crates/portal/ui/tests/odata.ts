import { vi } from "vitest";
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
export async function asOData(rest: Response): Promise<Response> {
  const body: unknown = await rest.json().catch(() => null);
  // A `204` of the REST door is the service's `200` with `null` (API/13 §2b).
  if (rest.ok) return odataValue(body, rest.status === 204 ? 200 : rest.status);
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

/** Each Portal door's REST door (API/13 §2b, T-7100; crates/portal/src/api/odata_portal.rs). */
const DOORS: Record<string, [string, string]> = {
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
};

const DOOR =
  /^\/api\/v1\/(?:Organization|projects\('((?:[^']|'')*)'\))\/JoinedContext\.Portal\.(portal_\w+|jc_person_(?:disable|enable|sign_out))(?:\(([^)]*)\))?$/;

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
  for (const [name, value] of Object.entries(params)) {
    if (path.includes(`{${name}}`))
      path = path.replace(`{${name}}`, encodeURIComponent(String(value)));
    else if (name === "body") body = JSON.stringify(value);
    else rest.searchParams.set(name, String(value));
  }
  rest.pathname = `/api/v1${path}`;
  return new Request(rest, {
    method: door[0],
    body,
    headers: body ? { "Content-Type": "application/json" } : undefined,
  });
}

const UNDER = /^\/api\/v1\/projects\('((?:[^']|'')*)'\)\/(\w+)$/;
const ORG_SET = /^\/api\/v1\/(\w+)$/;
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
): { rest: URL; one: boolean; organization?: boolean } | undefined {
  const at = new URL(String(url), "http://localhost");
  if (!(at.searchParams.get("$select") ?? "").split(",").includes("spec"))
    return undefined;
  const path = decodeURIComponent(at.pathname);
  const unquote = (text: string) => text.replaceAll("''", "'");
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
): Promise<Response> {
  if (!rest.ok) return asOData(rest);
  const body = (await rest.json()) as Record<string, unknown>;
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
