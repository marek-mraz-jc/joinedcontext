import type { Page, Request, Route } from "@playwright/test";
import { manifestAnswer, manifestAsRest } from "../tests/odata";

// T-7103: the pages read a kind's manifests through the Organization's OData service ($select of
// spec, status, metadata: api/odata.ts listManifests, readManifest). The server proves each answers
// as the REST read `GET /projects/{project}/{plural}[/{name}]` does (odata_org_tests
// a_manifests_spec_status_and_metadata_answer_as_the_rest_door), so a journey keeps its REST stubs:
// `manifestReadsAsRest(page)` before them hands each such read to the stub as its REST read and
// gives the page the stub's answer as the service's.

type Fulfil = Parameters<Route["fulfill"]>[0];

/** The stub's answer to the REST read, as the service answers the manifest read. */
async function asService(route: Route, options: Fulfil, one: boolean): Promise<void> {
  const status = options?.status ?? 200;
  const body = options?.json !== undefined ? JSON.stringify(options.json) : (options?.body ?? "null");
  const answer = await manifestAnswer(new Response(body, { status }), one);
  await route.fulfill({ status: answer.status, contentType: "application/json", body: await answer.text() });
}

type Handler = (route: Route, request: Request) => unknown;

function wrap(handler: Handler): Handler {
  return (route, request) => {
    const read = manifestAsRest(request.url());
    if (!read) return handler(route, request);
    const rest = read.rest.href;
    const asked = new Proxy(request, {
      get: (target, name) => (name === "url" ? () => rest : (Reflect.get(target, name) as unknown)),
    });
    const facade = new Proxy(route, {
      get: (target, name) => {
        if (name === "request") return () => asked;
        if (name === "fulfill") return (options: Fulfil) => asService(target, options, read.one);
        const member = Reflect.get(target, name) as unknown;
        return typeof member === "function" ? (member as (...a: unknown[]) => unknown).bind(target) : member;
      },
    });
    return handler(facade, asked);
  };
}

/**
 * Each `page.route` handler registered after this call meets a manifest read as its REST read;
 * `page.unroute` with the handler a spec registered takes its wrapper away.
 */
export function manifestReadsAsRest(page: Page): void {
  const wrapped = new WeakMap<Handler, Handler>();
  const route = page.route.bind(page);
  const unroute = page.unroute.bind(page);
  page.route = ((url: Parameters<Page["route"]>[0], handler: Handler, options?: Parameters<Page["route"]>[2]) => {
    const own = wrapped.get(handler) ?? wrap(handler);
    wrapped.set(handler, own);
    return route(url, own, options);
  }) as Page["route"];
  page.unroute = ((url: Parameters<Page["unroute"]>[0], handler?: Handler) =>
    unroute(url, handler && (wrapped.get(handler) ?? handler))) as Page["unroute"];
}
