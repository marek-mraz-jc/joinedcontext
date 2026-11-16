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
