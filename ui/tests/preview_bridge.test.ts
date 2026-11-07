/**
 * The preview bridge (T-0679, SDK-18, AP-63): a request from the frame this page created, under
 * the run's endpoint, is performed with the reviewer's session and answered, a write only when the
 * run's data needs name its operation; anything else is refused without a request, and the
 * frame's error reports go to the run.
 */
import { describe, expect, it, vi } from "vitest";
import { firstAsk, functionPathOf, grantedOperations, handleBridgeMessage, observationRelay, operationOf, PREVIEW_REASON, previewErrorOf, previewObservationOf, previewVersionOf, selectedSourceOf } from "../src/pages/apps/previewBridge";
import type { PreviewError } from "../src/pages/apps/previewBridge";

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const BASE = `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`;
const ID = encodeURIComponent("urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002");
const GRANTED = new Set(["queryEntity", "retrieveEntity", "updateAttrs"]);
const FUNCTIONS = "/api/v1/projects/helsinki/agent-runs/r1/functions/";

function request(overrides: Record<string, unknown> = {}) {
  return { kind: "jc-request", id: 7, method: "PATCH", path: `${BASE}/${ID}/attrs`, body: { status: { type: "Property", value: "closed" } }, ...overrides };
}

function frame() {
  return { postMessage: vi.fn() } as unknown as Window & { postMessage: ReturnType<typeof vi.fn> };
}

async function send(data: unknown, options: { source?: Window; from?: unknown; slug?: string; slugs?: string[]; fetchImpl?: unknown; report?: (e: PreviewError) => void } = {}) {
  const source = options.source ?? frame();
  const fetchImpl = (options.fetchImpl ?? vi.fn()) as typeof fetch;
  const outcome = await handleBridgeMessage(
    { source: "from" in options ? options.from : source, data },
    { slug: "slug" in options ? options.slug : SLUG, slugs: options.slugs, operations: GRANTED, functions: FUNCTIONS, source },
    options.report ?? (() => undefined),
    fetchImpl,
  );
  return { outcome, source: source as Window & { postMessage: ReturnType<typeof vi.fn> }, fetchImpl: fetchImpl as unknown as ReturnType<typeof vi.fn> };
}

describe("operationOf", () => {
  it("names the reads and the three writes under the endpoint", () => {
    expect(operationOf("GET", `${BASE}?type=BikeHireDockingStation&limit=20`, SLUG)).toBe("read");
    expect(operationOf("GET", `${BASE}/${ID}`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/ngsi-ld/v1/temporal/entities?type=A&timerel=after&timeAt=2026-09-01T00%3A00%3A00Z`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/schema/index.json`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/schema/v3/json-schema`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/access`, SLUG)).toBe("read");
    expect(operationOf("POST", BASE, SLUG)).toBe("createEntity");
    expect(operationOf("PATCH", `${BASE}/${ID}/attrs`, SLUG)).toBe("updateAttrs");
    expect(operationOf("DELETE", `${BASE}/${ID}`, SLUG)).toBe("deleteEntity");
  });

  it("names nothing outside the endpoint, through a traversal, or with a query on a write", () => {
    for (const path of [
      `${BASE}/../../../../v1/projects/helsinki`,
      `${BASE}/%2e%2e/%2E%2E/attrs`,
      `${BASE}/..%2F..%2F..%2F..%2F..%2Fv1%2Fprojects%2Fhelsinki/attrs`,
      `${BASE}/a%252Fb/attrs`,
      `${BASE}/a%5Cb/attrs`,
      `${BASE}\\..\\admin`,
      `${BASE}//attrs`,
      `/api/endpoint/${SLUG}/ngsi-ld/v1/subscriptions`,
      `/api/endpoint/other26charslugother26cha/ngsi-ld/v1/entities`,
      `/api/v1/projects/helsinki/agent-runs`,
      `https://evil.example/api/endpoint/${SLUG}/access`,
      `//evil.example/api/endpoint/${SLUG}/access`,
      `${BASE}#x`,
    ]) {
      expect(operationOf("GET", path, SLUG), path).toBeNull();
      expect(operationOf("PATCH", path, SLUG), path).toBeNull();
    }
    expect(operationOf("POST", `${BASE}?options=upsert`, SLUG)).toBeNull();
    expect(operationOf("DELETE", `${BASE}/${ID}?purge=true`, SLUG)).toBeNull();
    expect(operationOf("PUT", `${BASE}/${ID}`, SLUG)).toBeNull();
    expect(operationOf("POST", `/api/endpoint/${SLUG}/ngsi-ld/v1/entityOperations/delete`, SLUG)).toBeNull();
    expect(operationOf("GET", BASE, "bikes")).toBeNull();
    expect(operationOf("GET", 42, SLUG)).toBeNull();
  });
});

describe("functionPathOf", () => {
  it("sends a POST of a function name to the run's route with its query and nothing else", () => {
    expect(functionPathOf("POST", "/functions/summary", FUNCTIONS)).toBe(`${FUNCTIONS}summary`);
    expect(functionPathOf("POST", "/functions/count-2?types=A", FUNCTIONS)).toBe(`${FUNCTIONS}count-2?types=A`);
    for (const path of [
      "/functions/Summary",
      "/functions/../agent-runs",
      "/functions/%2e%2e",
      "/functions/a/b",
      "/functions/",
      `/functions/${"a".repeat(41)}`,
      "/functions/summary#x",
      "//evil.example/functions/summary",
      "/api/functions/summary",
    ]) {
      expect(functionPathOf("POST", path, FUNCTIONS), path).toBeNull();
    }
    expect(functionPathOf("GET", "/functions/summary", FUNCTIONS)).toBeNull();
  });
});

describe("grantedOperations", () => {
  it("reads the operations of every data need and nothing else", () => {
    expect(grantedOperations([{ operations: ["queryEntity", "updateAttrs"] }, { operations: ["createEntity", 3] }])).toEqual(
      new Set(["queryEntity", "updateAttrs", "createEntity"]),
    );
    expect(grantedOperations(null)).toEqual(new Set());
    expect(grantedOperations([null, { operations: "updateAttrs" }])).toEqual(new Set());
  });
});

describe("handleBridgeMessage", () => {
  it("performs a read with the session and posts the answer back", async () => {
    const rows = [{ id: "urn:ngsi-ld:A:hel.fi:h:1", type: "A" }];
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify(rows), { status: 200, headers: { "content-type": "application/ld+json" } }));
    const { outcome, source } = await send(request({ method: "GET", path: `${BASE}?type=A`, body: undefined }), { fetchImpl });
    expect(outcome).toBe("forwarded");
    const [path, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${BASE}?type=A`);
    expect(init.method).toBe("GET");
    expect(init.credentials).toBe("same-origin");
    expect(init.body).toBeUndefined();
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBeUndefined();
    expect((init.headers as Record<string, string>)["JC-Reason"]).toBeUndefined();
    expect(source.postMessage).toHaveBeenCalledWith({ kind: "jc-response", id: 7, status: 200, body: rows }, "*");
  });

  // DM-113: a preview writes with the reviewer's Portal session, so each write names why: the
  // application's own reason when the gateway would take it, else the preview's, percent-encoded.
  it("sends the frame's reason with a write, or the preview's when it names none or a bad one", async () => {
    const fetchImpl = vi.fn(async () => new Response(null, { status: 204 }));
    const reasonOf = (call: number) => ((fetchImpl.mock.calls[call] as unknown as [string, RequestInit])[1].headers as Record<string, string>)["JC-Reason"];
    await send(request({ reason: "Dock closed\r\nX-Evil: 1" }), { fetchImpl });
    expect(reasonOf(0)).toBe(encodeURIComponent("Dock closed\r\nX-Evil: 1"));
    await send(request(), { fetchImpl });
    expect(reasonOf(1)).toBe(encodeURIComponent(PREVIEW_REASON));
    const recordOf = (call: number) => ((fetchImpl.mock.calls[call] as unknown as [string, RequestInit])[1].headers as Record<string, string>)["JC-Record"];
    expect([0, 1].map(recordOf)).toEqual(["version", "version"]);
    for (const bad of ["  ", "x".repeat(501), 42]) {
      await send(request({ reason: bad }), { fetchImpl });
    }
    expect([2, 3, 4].map(reasonOf)).toEqual(Array(3).fill(encodeURIComponent(PREVIEW_REASON)));
  });

  it("performs a granted write with the CSRF header and hands back the endpoint's answer", async () => {
    document.cookie = "__Host-jc_csrf=csrf-abc; Secure; path=/";
    const problem = { type: "about:blank", title: "Forbidden", status: 403, detail: "no policy grants this write" };
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(problem), { status: 403, headers: { "content-type": "application/problem+json" } }));
    const source = frame();
    expect((await send(request(), { source, fetchImpl })).outcome).toBe("forwarded");
    const [path, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${BASE}/${ID}/attrs`);
    expect(init.method).toBe("PATCH");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-abc");
    expect(JSON.parse(init.body as string)).toEqual({ status: { type: "Property", value: "closed" } });
    expect(source.postMessage).toHaveBeenLastCalledWith({ kind: "jc-response", id: 7, status: 204, body: null }, "*");

    await send(request({ id: 8 }), { source, fetchImpl });
    expect(source.postMessage).toHaveBeenLastCalledWith({ kind: "jc-response", id: 8, status: 403, body: problem }, "*");
  });

  it("calls the run's function with the session and hands back its answer", async () => {
    document.cookie = "__Host-jc_csrf=csrf-fn; Secure; path=/";
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify({ types: [] }), { status: 201, headers: { "content-type": "application/json" } }));
    const { outcome, source } = await send(request({ method: "POST", path: "/functions/summary", body: { types: ["A"] } }), { fetchImpl });
    expect(outcome).toBe("forwarded");
    const [path, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${FUNCTIONS}summary`);
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-fn");
    expect(JSON.parse(init.body as string)).toEqual({ types: ["A"] });
    expect(source.postMessage).toHaveBeenCalledWith({ kind: "jc-response", id: 7, status: 201, body: { types: [] } }, "*");

    const refused = await send(request({ method: "POST", path: "/functions/../../cancel", body: {} }));
    expect(refused.outcome).toBe("refused");
    expect(refused.fetchImpl).not.toHaveBeenCalled();
  });

  it("answers a write the data needs do not name with 403 and sends no request", async () => {
    const { outcome, source, fetchImpl } = await send(request({ method: "DELETE", path: `${BASE}/${ID}`, body: undefined }));
    expect(outcome).toBe("refused");
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(source.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "jc-response", id: 7, status: 403, body: expect.objectContaining({ detail: "The run's data needs do not name deleteEntity." }) }),
      "*",
    );
  });

  it("refuses a traversal, another slug and a malformed write without a request", async () => {
    for (const data of [
      request({ path: `${BASE}/..%2F..%2F..%2F..%2F..%2Fv1%2Fprojects%2Fhelsinki/attrs` }),
      request({ path: `/api/endpoint/other26charslugother26cha/ngsi-ld/v1/entities/${ID}/attrs` }),
      request({ body: "text" }),
      request({ kind: "kit-write" }),
      request({ id: "7" }),
      null,
    ]) {
      const { outcome, fetchImpl } = await send(data);
      expect(outcome, JSON.stringify(data)).toBe("refused");
      expect(fetchImpl).not.toHaveBeenCalled();
    }
  });

  it("ignores another window, and every message when the run has no slug", async () => {
    const { outcome, source, fetchImpl } = await send(request(), { from: frame() });
    expect(outcome).toBe("ignored");
    expect((await send(request(), { slug: undefined })).outcome).toBe("ignored");
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(source.postMessage).not.toHaveBeenCalled();
  });

  it("relays the frame's error reports, bounded, and a foreign window's not at all", async () => {
    const report = vi.fn();
    const long = "x".repeat(3000);
    expect((await send({ kind: "jc-error", message: long, file: "src/App.tsx", line: 12 }, { report })).outcome).toBe("relayed");
    expect(report).toHaveBeenCalledWith({ message: "x".repeat(2000), file: "src/App.tsx", line: 12 });
    expect((await send({ kind: "jc-error", message: "boom" }, { report, from: frame() })).outcome).toBe("ignored");
    expect(report).toHaveBeenCalledTimes(1);
    expect(previewErrorOf({ kind: "jc-error", message: "boom", line: 0, file: 3 })).toEqual({ message: "boom" });
    expect(previewErrorOf({ kind: "jc-error", message: "  " })).toBeNull();
  });
});

describe("preview observations (SDK-27)", () => {
  const observation = {
    kind: "jc-observation",
    version: 2,
    pages: [
      { label: "Overview", text: "Stations 5", rows: [], h1: 1 },
      { label: "Stations", text: "Kaivopuisto 7", rows: [5] },
    ],
    failedRequests: [{ path: "/functions/summary", status: 500 }],
  };

  async function observe(data: unknown, options: { from?: unknown; version?: number } = {}) {
    const source = frame();
    const seen = vi.fn();
    const outcome = await handleBridgeMessage(
      { source: "from" in options ? options.from : source, data },
      { slug: SLUG, operations: GRANTED, functions: FUNCTIONS, source, observe: seen, version: options.version },
      () => undefined,
      vi.fn() as unknown as typeof fetch,
    );
    return { outcome, seen };
  }

  it("hands a valid observation of the created frame to the run, and a foreign window's not at all", async () => {
    const { outcome, seen } = await observe(observation);
    expect(outcome).toBe("relayed");
    expect(seen).toHaveBeenCalledWith({ version: 2, pages: observation.pages, failedRequests: observation.failedRequests });

    const foreign = await observe(observation, { from: frame() });
    expect(foreign.outcome).toBe("ignored");
    expect(foreign.seen).not.toHaveBeenCalled();
  });

  it("drops an observation that breaks a bound of the route, whole", async () => {
    for (const bad of [
      { ...observation, version: 0 },
      { ...observation, pages: [] },
      { ...observation, pages: Array.from({ length: 21 }, () => observation.pages[0]) },
      { ...observation, pages: [{ label: "x".repeat(121), text: "", rows: [] }] },
      { ...observation, pages: [{ label: "A", text: "x".repeat(20_001), rows: [] }] },
      { ...observation, pages: [{ label: "A", text: "", rows: Array.from({ length: 51 }, () => 1) }] },
      { ...observation, pages: [{ label: "A", text: "", rows: [-1] }] },
      { ...observation, pages: [{ label: "A", text: "", rows: [], h1: 51 }] },
      { ...observation, pages: [{ label: "A", text: "", rows: [], h1: 1.5 }] },
      { ...observation, pages: [{ label: "A", text: "", rows: [], h1: "1" }] },
      { ...observation, failedRequests: [{ path: "/functions/summary", status: 302 }] },
      { ...observation, failedRequests: [{ path: "x".repeat(257), status: 500 }] },
      { ...observation, failedRequests: Array.from({ length: 51 }, () => observation.failedRequests[0]) },
    ]) {
      const { outcome, seen } = await observe(bad);
      expect(outcome, JSON.stringify(bad).slice(0, 80)).toBe("refused");
      expect(seen).not.toHaveBeenCalled();
    }
  });

  it("takes the framed URL's version when the frame does not know its own", async () => {
    const withoutVersion: Record<string, unknown> = { ...observation };
    delete withoutVersion.version;
    expect((await observe(withoutVersion)).outcome).toBe("refused");
    const { outcome, seen } = await observe(withoutVersion, { version: 4 });
    expect(outcome).toBe("relayed");
    expect(seen.mock.calls[0][0].version).toBe(4);
    expect(previewVersionOf("/api/v1/projects/p/agent-runs/r/preview?v=7")).toBe(7);
    expect(previewVersionOf("/api/v1/projects/p/agent-runs/r/preview")).toBeUndefined();
  });

  it("posts the first observation of each version to the run once", async () => {
    const fetchImpl = vi.fn(async () => new Response(null, { status: 409 }));
    const relay = observationRelay("helsinki", "r1", fetchImpl as unknown as typeof fetch);
    const first = previewObservationOf(observation)!;
    relay(first);
    relay({ ...first, pages: [{ label: "Later", text: "", rows: [] }] });
    relay({ ...first, version: 3 });
    await Promise.resolve();
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/projects/helsinki/agent-runs/r1/preview-observations");
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body as string)).toEqual({ version: 2, pages: observation.pages, failedRequests: observation.failedRequests });
    expect((fetchImpl.mock.calls[1] as unknown as [string, RequestInit])[1].body).toContain('"version":3');
  });
});

describe("asking the frame to observe (SDK-27)", () => {
  const ready = { kind: "jc-ready" };
  const read = { kind: "jc-request", id: 1, method: "GET", path: `${BASE}?type=BikeHireDockingStation` };
  const ok = () => vi.fn(async () => new Response("[]", { status: 200, headers: { "content-type": "application/json" } }));

  async function tell(data: unknown, source: Window, options: { live?: boolean; version?: number; ask?: (version: number) => boolean; from?: unknown } = {}) {
    return handleBridgeMessage(
      { source: "from" in options ? options.from : source, data },
      { slug: SLUG, operations: GRANTED, functions: FUNCTIONS, source, version: options.version, live: options.live, ask: options.ask },
      () => undefined,
      ok() as unknown as typeof fetch,
    );
  }
  const asks = (source: ReturnType<typeof frame>) =>
    source.postMessage.mock.calls.filter(([message]) => (message as { kind?: string }).kind === "jc-observe");

  it("asks a ready frame of a live run once per version, however often it loads", async () => {
    const ask = firstAsk("helsinki/r1", new Set());
    const first = frame();
    expect(await tell(ready, first, { live: true, version: 2, ask })).toBe("relayed");
    expect(asks(first)).toEqual([[{ kind: "jc-observe", version: 2 }, "*"]]);

    // The application closed and opened again: the same version is not walked twice.
    const reopened = frame();
    await tell(ready, reopened, { live: true, version: 2, ask });
    await tell(read, reopened, { live: true, version: 2, ask });
    expect(asks(reopened)).toEqual([]);

    // A new version is asked for, and a first request counts as ready when `jc-ready` is lost.
    const next = frame();
    expect(await tell(read, next, { live: true, version: 3, ask })).toBe("forwarded");
    expect(asks(next)).toEqual([[{ kind: "jc-observe", version: 3 }, "*"]]);
  });

  it("never asks for an ended run, a frame without a version, or a foreign window", async () => {
    const ask = vi.fn(firstAsk("helsinki/r1", new Set()));
    const source = frame();
    await tell(ready, source, { live: false, version: 2, ask });
    await tell(read, source, { live: false, version: 2, ask });
    await tell(ready, source, { live: true, ask });
    expect(await tell(ready, source, { live: true, version: 2, ask, from: frame() })).toBe("ignored");
    expect(asks(source)).toEqual([]);
    expect(ask).not.toHaveBeenCalled();
  });

  it("remembers each run's versions apart", () => {
    const memory = new Set<string>();
    expect(firstAsk("helsinki/r1", memory)(2)).toBe(true);
    expect(firstAsk("helsinki/r1", memory)(2)).toBe(false);
    expect(firstAsk("helsinki/r2", memory)(2)).toBe(true);
  });
});

describe("a run of several endpoints (AP-44, SDK-18)", () => {
  const KPIS = "q3mzkq2v7w5ayxcbn4ltdj6hof";
  const ok = () => vi.fn(async () => new Response("[]", { status: 200, headers: { "content-type": "application/json" } }));

  it("forwards a read under any slug of the run", async () => {
    const fetchImpl = ok();
    const { outcome } = await send(
      { kind: "jc-request", id: 1, method: "GET", path: `/api/endpoint/${KPIS}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator` },
      { slugs: [KPIS], fetchImpl },
    );
    expect(outcome).toBe("forwarded");
    expect(fetchImpl).toHaveBeenCalledOnce();
  });

  it("still refuses a slug that is not the run's", async () => {
    const fetchImpl = ok();
    const { outcome } = await send(
      { kind: "jc-request", id: 1, method: "GET", path: `/api/endpoint/${KPIS}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator` },
      { fetchImpl },
    );
    expect(outcome).toBe("refused");
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

describe("pointing at an element (SDK-46)", () => {
  const pick = (data: unknown, from?: unknown) => {
    const source = frame();
    const select = vi.fn();
    const outcome = handleBridgeMessage(
      { source: from === undefined ? source : from, data },
      { slug: SLUG, operations: GRANTED, functions: FUNCTIONS, source, select },
      vi.fn(),
      vi.fn() as unknown as typeof fetch,
    );
    return { outcome, select };
  };

  it("passes the position the frame it created names, and nothing else", async () => {
    const { outcome, select } = pick({ kind: "jc-select", src: "src/pages/Card.tsx:4", text: "Station 7" });
    expect(await outcome).toBe("relayed");
    expect(select).toHaveBeenCalledWith("src/pages/Card.tsx:4");
  });

  it("ignores a jc-select from another frame", async () => {
    const { outcome, select } = pick({ kind: "jc-select", src: "src/pages/Card.tsx:4" }, frame());
    expect(await outcome).toBe("ignored");
    expect(select).not.toHaveBeenCalled();
  });

  it("refuses a position that is not an interface file and a line", async () => {
    for (const src of ["src/pages/Card.tsx", "src/../x.tsx:1", "functions/a.ts:2", "src/a.tsx:0", "src/.hidden.tsx:1", "src/a.tsx:1 Station 7", 4]) {
      const { outcome, select } = pick({ kind: "jc-select", src });
      expect(await outcome, String(src)).toBe("refused");
      expect(select).not.toHaveBeenCalled();
    }
    expect(selectedSourceOf({ kind: "jc-select", src: `src/${"a/".repeat(200)}b.tsx:1` })).toBeNull();
  });

  it("refuses a selection on a page that takes none", async () => {
    const source = frame();
    const outcome = await handleBridgeMessage(
      { source, data: { kind: "jc-select", src: "src/App.tsx:3" } },
      { slug: SLUG, operations: GRANTED, functions: FUNCTIONS, source },
      vi.fn(),
    );
    expect(outcome).toBe("refused");
  });
});
