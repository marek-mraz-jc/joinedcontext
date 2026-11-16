import type { Page, Request, Route } from "@playwright/test";
import { answerShape, asOData, callsOperation, doorAsRest, manifestAnswer, manifestAsRest, proposalAsRest } from "../tests/odata";

// T-7103: the pages read a kind's manifests through the Organization's OData service ($select of
// spec, status, metadata: api/odata.ts listManifests, readManifest). The server proves each answers
// as the REST read `GET /projects/{project}/{plural}[/{name}]` does (odata_org_tests
// a_manifests_spec_status_and_metadata_answer_as_the_rest_door), so a journey keeps its REST stubs:
// `manifestReadsAsRest(page)` before them hands each such read to the stub as its REST read and
// gives the page the stub's answer as the service's.

type Fulfil = Parameters<Route["fulfill"]>[0];

/** The stub's answer to the REST read, as the service answers the manifest read. */
async function asService(route: Route, options: Fulfil, one: boolean, organization?: boolean, row?: boolean): Promise<void> {
  const status = options?.status ?? 200;
  const body = options?.json !== undefined ? JSON.stringify(options.json) : (options?.body ?? "null");
  const answer = await manifestAnswer(new Response(body, { status }), one, organization, row);
  await route.fulfill({ status: answer.status, contentType: "application/json", body: await answer.text() });
}

type Handler = (route: Route, request: Request) => unknown;

/** The REST request a stub meets instead of the service's, and how its answer goes back. */
interface AsRest {
  url: string;
  method?: string;
  body?: string | null;
  fulfil: (route: Route, options: Fulfil) => Promise<void>;
}
type Translate = (request: Request) => Promise<AsRest | undefined>;

const manifestRead: Translate = async (request) => {
  const read = manifestAsRest(request.url());
  return read && { url: read.rest.href, fulfil: (route, options) => asService(route, options, read.one, read.organization, read.row) };
};

/** A Portal door (api/odata.ts `door`, `doorAction`) as its REST door, its answer as the service wraps it. */
const portalDoor: Translate = async (request) => {
  const body = request.postData();
  const asked = new Request(request.url(), { method: request.method(), body: body ?? undefined });
  const rest = await doorAsRest(asked);
  if (!rest) return undefined;
  // An operation whose answer is not its REST door's (tests/odata.ts ANSWERS), e.g. jc_workspace_propose.
  const shape = answerShape(asked);
  const text = rest.body ? await rest.text() : null;
  return {
    url: rest.url,
    method: rest.method,
    body: text,
    fulfil: async (route, options) => {
      const status = options?.status ?? 200;
      const sent = options?.json !== undefined ? JSON.stringify(options.json) : (options?.body ?? "null");
      const answer = await asOData(new Response(status === 204 ? null : String(sent), { status }), shape);
      await route.fulfill({ status: answer.status, contentType: "application/json", body: await answer.text() });
    },
  };
};

/** The request as the fetch API holds it, for the translations in tests/odata.ts. */
async function sent(request: Request): Promise<globalThis.Request> {
  return new globalThis.Request(request.url(), {
    method: request.method(),
    headers: await request.allHeaders(),
    body: request.method() === "POST" ? (request.postData() ?? undefined) : undefined,
  });
}

/**
 * T-7073: a `propose_*` action as the REST write it stands for (POST, PUT, PATCH or DELETE with
 * dryRun and confirm as the door's query); a success goes back as the door's body, a refusal as the
 * service's error.
 */
const proposal: Translate = async (request) => {
  if (!request.url().includes("/JoinedContext.Portal.propose_")) return undefined;
  const rest = await proposalAsRest(await sent(request));
  if (!rest) return undefined;
  const text = rest.body ? await rest.text() : null;
  return {
    url: rest.url,
    method: rest.method,
    body: text,
    fulfil: async (route, options) => {
      const status = options?.status ?? 200;
      const body = options?.json !== undefined ? JSON.stringify(options.json) : (options?.body ?? "null");
      const answer = status >= 300 ? await asOData(new Response(body, { status })) : new Response(body, { status });
      await route.fulfill({ status: answer.status, contentType: "application/json", headers: options?.headers, body: await answer.text() });
    },
  };
};

/** T-7073: a registry operation bound to a project (jc_syncsource_*, jc_ckan_status, ...) as its REST door. */
const operation: Translate = async (request) => {
  const asked = await sent(request);
  return callsOperation(asked) ? portalDoor(request) : undefined;
};

/** A manifest read, a proposal or a registry operation, whichever the request is. */
const manifestCall: Translate = async (request) =>
  (await manifestRead(request)) ?? (await proposal(request)) ?? (await operation(request));

function wrap(handler: Handler, translate: Translate): Handler {
  return async (route, request) => {
    const read = await translate(request);
    if (!read) return handler(route, request);
    const members: Record<string, () => unknown> = { url: () => read.url };
    if (read.method) members.method = () => read.method;
    if (read.body !== undefined) {
      members.postData = () => read.body;
      members.postDataJSON = () => (read.body ? (JSON.parse(read.body) as unknown) : null);
    }
    const asked = new Proxy(request, {
      get: (target, name) => (typeof name === "string" && name in members ? members[name] : (Reflect.get(target, name) as unknown)),
    });
    const facade = new Proxy(route, {
      get: (target, name) => {
        if (name === "request") return () => asked;
        if (name === "fulfill") return (options: Fulfil) => read.fulfil(target, options);
        const member = Reflect.get(target, name) as unknown;
        return typeof member === "function" ? (member as (...a: unknown[]) => unknown).bind(target) : member;
      },
    });
    return handler(facade, asked);
  };
}

/**
 * Each `page.route` handler registered after this call meets a manifest read as its REST read, and
 * a proposal or a registry operation as its REST write or door (T-7073);
 * `page.unroute` with the handler a spec registered takes its wrapper away.
 */
export function manifestReadsAsRest(page: Page): void {
  asRest(page, manifestCall);
}

/**
 * The same for the Portal doors `tests/odata.ts` DOORS names (T-7110): a spec whose stub answers
 * a door's REST address calls this before its `page.route`; a spec answering the service itself
 * does not.
 */
export function doorsAsRest(page: Page): void {
  asRest(page, portalDoor);
}

function asRest(page: Page, translate: Translate): void {
  const wrapped = new WeakMap<Handler, Handler>();
  const route = page.route.bind(page);
  const unroute = page.unroute.bind(page);
  page.route = ((url: Parameters<Page["route"]>[0], handler: Handler, options?: Parameters<Page["route"]>[2]) => {
    const own = wrapped.get(handler) ?? wrap(handler, translate);
    wrapped.set(handler, own);
    return route(url, own, options);
  }) as Page["route"];
  page.unroute = ((url: Parameters<Page["unroute"]>[0], handler?: Handler) =>
    unroute(url, handler && (wrapped.get(handler) ?? handler))) as Page["unroute"];
}
