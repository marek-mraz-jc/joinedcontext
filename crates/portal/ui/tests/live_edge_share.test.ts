/**
 * T-6404: a sweep worker hands a fetched bundle chunk to its later contexts, decoded, and only a
 * whole 200; it counts the edge's requests per minute.
 */
// covers: e2e/live/edgeShare.ts
import type { BrowserContext, Route } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { keepable, minuteOf, replayHeaders, shareEdge } from "../e2e/live/edgeShare";

type Handler = (route: Route) => Promise<void>;

/** A context that keeps the route handler shareEdge installs. */
async function sharedContext(): Promise<Handler> {
  let handler: Handler | undefined;
  const context = { on: () => undefined, route: async (_match: unknown, h: Handler) => void (handler = h) } as unknown as BrowserContext;
  await shareEdge(context);
  if (!handler) throw new Error("shareEdge routed nothing");
  return handler;
}

/** A route for `url` whose fetch answers `status`, recording fetches and what it was fulfilled with. */
function fakeRoute(url: string, status: number, log: { fetched: number; fulfilled: unknown[] }): Route {
  return {
    request: () => ({ url: () => url }),
    fetch: async () => {
      log.fetched += 1;
      return { status: () => status, headers: () => ({ "content-encoding": "br", "content-type": "text/javascript" }), body: async () => Buffer.from("x") };
    },
    fulfill: async (answer: unknown) => void log.fulfilled.push(answer),
    continue: async () => undefined,
  } as unknown as Route;
}

describe("edgeShare", () => {
  it("drops the headers of the encoded body and keeps the rest", () => {
    expect(
      replayHeaders({ "Content-Encoding": "br", "content-length": "10", "transfer-encoding": "chunked", "content-type": "text/javascript", etag: "x" }),
    ).toEqual({ "content-type": "text/javascript", etag: "x" });
  });

  it("keeps only a 200, never the edge's 429 or an error", () => {
    const asset = (status: number) => ({ status, headers: {}, body: Buffer.from("") });
    expect(keepable(asset(200))).toBe(true);
    expect([429, 502, 404].map((status) => keepable(asset(status)))).toEqual([false, false, false]);
  });

  it("counts by UTC minute", () => {
    expect(minuteOf(Date.UTC(2026, 10, 2, 1, 7, 59, 999))).toBe("2026-11-02T01:07");
  });

  it("fetches a chunk once for every context of the worker, and asks again after a 429", async () => {
    const log = { fetched: 0, fulfilled: [] as unknown[] };
    const [first, second] = [await sharedContext(), await sharedContext()];
    await first(fakeRoute("https://portal.x/assets/index-a.js", 200, log));
    await second(fakeRoute("https://portal.x/assets/index-a.js", 200, log));
    expect(log.fetched).toBe(1);
    expect(log.fulfilled).toEqual([1, 2].map(() => ({ status: 200, headers: { "content-type": "text/javascript" }, body: Buffer.from("x") })));
    await first(fakeRoute("https://portal.x/assets/index-b.js", 429, log));
    await second(fakeRoute("https://portal.x/assets/index-b.js", 429, log));
    expect(log.fetched).toBe(3);
  });
});
