// T-6812 (ADR-N-070, API/13 §2a, §2b): the Portal UI's one client of its Organization's OData
// service. Reads of sets, the functions and the actions go through here; the REST doors in
// client.ts shrink page group by page group under tests/odata_ratchet.test.ts.
import { odata, ODataError, originTransport, type ODataModel } from "@joinedcontext/sdk";
import { ApiError, loginRedirectUrl, type WireProblem } from "./client";
import type { operations } from "./schema";
import { announceSessionEnded } from "./sessionEnded";
import { announceStepUp, stepUpUrl } from "./stepUp";

/** The service root: the Portal serves its own Organization at its origin (API/15 §2). */
export const ODATA_ROOT = "/api/v1/";

const SECOND_FACTOR = "https://joinedcontext.com/errors/second-factor-required";

/** The problem type of an answer, an OData error object's or a REST problem's. */
function typeOf(body: unknown): unknown {
  if (typeof body !== "object" || body === null) return undefined;
  const error = (body as { error?: { type?: unknown } }).error;
  return error?.type ?? (body as { type?: unknown }).type;
}

/**
 * `fetch` with what every Portal request has: a 401 ends the session where the person is, and a
 * decision that needs a second factor opens the step-up dialog (T-2747, T-5567), as api/client.ts
 * does for the REST doors.
 */
async function portalFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  // One `Request` with an absolute address, as the REST client sends: jsdom has no base for a path.
  const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
  const response = await globalThis.fetch(request);
  const here = `${window.location.pathname}${window.location.search}`;
  if (response.status === 401 && window.location.pathname !== "/login") {
    announceSessionEnded(loginRedirectUrl(here));
  } else if (response.status === 403) {
    const body: unknown = await response
      .clone()
      .json()
      .catch(() => null);
    if (typeOf(body) === SECOND_FACTOR) announceStepUp(stepUpUrl(here));
  }
  return response;
}

const client = odata(ODATA_ROOT, { transport: originTransport(portalFetch) });

/**
 * A refusal of the service as every page reads one: an ApiError whose problem holds the OData
 * error's API/09 members (`type`, `title`, `status`, `message` as `detail`), so a page shows its
 * refusal, empty and retry states unchanged.
 */
export function toApiError(error: unknown): unknown {
  if (!(error instanceof ODataError)) return error;
  const problem: WireProblem = {
    type: error.type,
    title: error.title,
    status: error.status,
    detail: error.message,
  };
  return new ApiError(error.status, error.message, problem, error.requestId);
}

async function refusedAs<T>(answer: Promise<T>): Promise<T> {
  try {
    return await answer;
  } catch (error) {
    throw toApiError(error);
  }
}

type Functions = ODataModel.Functions;
type Actions = ODataModel.Actions;

/** The answer an operation documents in the REST contract, until T-7075 retires the REST types. */
type Answer<O extends keyof operations> = operations[O] extends {
  responses: { 200: { content: { "application/json": infer T } } };
}
  ? T
  : never;

/** Each function the UI calls and the shape of its answer: the same handler serves its REST door. */
interface Answers {
  jc_access_explore: Answer<"explore_access">;
  jc_access_simulate: Answer<"simulate_access">;
  jc_change_history: Answer<"list_change_history">;
  jc_endpoint_preview: Answer<"preview_endpoint">;
  jc_serviceaccount_access: Answer<"account_access">;
}

/**
 * A bound function of `project`, e.g. `call("jc_change_history", "air", { page: 1 })`: its value
 * is the operation's answer.
 */
export function call<F extends keyof Functions & string>(
  name: F,
  project: string,
  params: Functions[F],
): Promise<F extends keyof Answers ? Answers[F] : unknown> {
  return refusedAs(client.call(name, project, params)) as Promise<F extends keyof Answers ? Answers[F] : unknown>;
}

/** Each Administration page's reading of `jc_organization_read` and its answer (T-7098). */
interface Readings {
  setup: Answer<"get_setup">;
  health: Answer<"get_health">;
  limits: Answer<"get_limits">;
  brokers: Answer<"list_brokers">;
  agents: Answer<"list_agents">;
  costs: Answer<"get_costs">;
  addresses: Answer<"list_addresses">;
  dataAccess: Answer<"get_data_access">;
  widgetOptions: Answer<"list_options">;
  personReadable: Answer<"get_person_readable">;
}

type ReadingParams = Omit<Functions["jc_organization_read"], "aspect">;

/**
 * One reading of the Organization's Administration pages, bound to its `Organization` singleton,
 * e.g. `organizationReading("costs", { month: "2026-09" })`: the same handler serves its REST door.
 */
export function organizationReading<A extends keyof Readings>(aspect: A, params: ReadingParams = {}): Promise<Readings[A]> {
  return refusedAs(client.organizationCall("jc_organization_read", { ...params, aspect })) as Promise<Readings[A]>;
}

/**
 * A bound action on `bound` (`projects('air')`, an entity, a Change), with `ifMatch` the entity's
 * `@odata.etag` where the action changes it.
 */
export function act<A extends keyof Actions & string>(name: A, bound: string, params: Actions[A], ifMatch?: string): Promise<unknown> {
  return refusedAs(client.act(name, bound, params, ifMatch));
}

/** The address an action of project `project` is bound to. */
export function projectBound(project: string): string {
  return `projects('${project.replaceAll("'", "''")}')`;
}
