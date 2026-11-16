// The Organization's OData service as a page test meets it (T-6812): which bound function or
// action a request calls, with what, and the answer the service wraps an operation's value in.

/** One call of a bound function or action, as the SDK client sends it. */
export interface ODataCall {
  project: string;
  name: string;
  params: Record<string, unknown>;
}

const BOUND = /^\/api\/v1\/projects\('((?:[^']|'')*)'\)\/JoinedContext\.Portal\.(\w+)(?:\(([^)]*)\))?$/;

/** A literal of an alias: text in quotes, a number, a boolean, `null` or JSON. */
function literal(text: string): unknown {
  if (text.startsWith("'") && text.endsWith("'")) return text.slice(1, -1).replaceAll("''", "'");
  return JSON.parse(text) as unknown;
}

/**
 * The function or action `request` calls, or nothing for any other request: a function's
 * arguments come from its aliases, an action's from its JSON body (`body`).
 */
export function odataCall(url: string | URL, body?: unknown): ODataCall | undefined {
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
  return JSON.stringify({ "@odata.context": "/api/v1/$metadata#Edm.Untyped", value });
}

/** The service's answer to a function or action. */
export function odataValue(value: unknown, status = 200): Response {
  return new Response(odataBody(value), {
    status,
    headers: { "Content-Type": "application/json", "OData-Version": "4.01" },
  });
}

/** The service's refusal: the OData error object with API/09's members inside (API/13 §2a). */
export function odataError(status: number, code: string, message: string): Response {
  return new Response(
    JSON.stringify({ error: { code, message, type: `https://joinedcontext.com/errors/${code}`, title: "Error", status } }),
    { status, headers: { "Content-Type": "application/json" } },
  );
}

/**
 * A REST door's answer as the service gives the same operation's: a success's body under `value`,
 * a problem document as the OData error object carrying its members.
 */
export async function asOData(rest: Response): Promise<Response> {
  const body: unknown = await rest.json().catch(() => null);
  if (rest.ok) return odataValue(body, rest.status);
  const problem = (body ?? {}) as { type?: string; detail?: string; title?: string };
  const code = problem.type?.split("/").pop() ?? "internal-error";
  return odataError(rest.status, code, problem.detail ?? problem.title ?? `HTTP ${rest.status}`);
}

const READING = /^\/api\/v1\/Organization\/JoinedContext\.Portal\.jc_organization_read\(([^)]*)\)$/;

/** The REST door of each Administration reading: the handler `jc_organization_read` runs (T-7098). */
const READING_DOORS: Record<string, (params: Record<string, string>) => string> = {
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
  personReadable: (params) => `/api/v1/organization/people/${encodeURIComponent(params.person ?? "")}/readable`,
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
    if (name !== "aspect" && name !== "person") rest.searchParams.set(name, value);
  }
  return rest;
}
