// T-7706: knowledge.ts edges: upload and chat with a CSRF cookie, an empty or unreadable answer, tables, charts
// and visualization specs that must be refused, and the link hosts of a malformed origin list.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import { askAssistant, cellText, dataChart, dataTable, linkHostsOf, uploadFile, visualOf } from "../src/pages/knowledge/knowledge";
import type { KnowledgeSourceRow } from "../src/pages/knowledge/knowledge";

const GOOD = {
  $schema: "https://vega.github.io/schema/vega-lite/v6.json",
  data: { name: "source" },
  mark: { type: "line" },
  encoding: { x: { field: "time", type: "temporal" }, y: { field: "value", type: "quantitative" } },
};
const ROWS = [{ time: "2026-10-18T15:00:00Z", value: 14.2 }];

let sent: Request[] = [];
function answer(response: () => Response) {
  sent = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      sent.push(input as Request);
      return Promise.resolve(response());
    }),
  );
}

beforeEach(() => {
  document.cookie = "__Host-jc_csrf=tok%20en; Secure; path=/";
});
afterEach(() => {
  vi.unstubAllGlobals();
  document.cookie = "__Host-jc_csrf=; Secure; path=/; max-age=0";
});

describe("uploadFile", () => {
  it("sends the bytes with the CSRF token and returns the stored file", async () => {
    answer(() => new Response(JSON.stringify({ id: 4, name: "a.txt" }), { status: 201, headers: { "Content-Type": "application/json" } }));
    const file = await uploadFile("helsinki", "web", new Blob(["hi"]), "a b.txt");
    expect(file).toMatchObject({ id: 4 });
    expect(sent[0].headers.get("x-csrf-token")).toBe("tok en");
    expect(sent[0].url).toContain("/knowledge/sources/web/files?name=a%20b.txt");
  });

  it("refuses an answer that is not JSON or is empty", async () => {
    answer(() => new Response("not json", { status: 200 }));
    await expect(uploadFile("helsinki", "web", new Blob(["x"]), "a.txt")).rejects.toThrow(/answered nothing/);
    answer(() => new Response("null", { status: 200, headers: { "Content-Type": "application/json" } }));
    await expect(uploadFile("helsinki", "web", new Blob(["x"]), "a.txt")).rejects.toBeInstanceOf(ApiError);
  });

  it("raises the Portal's sentence on a refusal", async () => {
    answer(() => new Response(JSON.stringify({ detail: "too large" }), { status: 413, headers: { "Content-Type": "application/json" } }));
    await expect(uploadFile("helsinki", "web", new Blob(["x"]), "a.txt")).rejects.toMatchObject({ status: 413, message: expect.stringContaining("too large") });
  });
});

describe("askAssistant", () => {
  it("sends the CSRF token and reads the stream's events", async () => {
    const events: string[] = [];
    answer(() => new Response('event: token\ndata: {"text":"hi"}\n\n', { status: 200, headers: { "Retry-After": "7" } }));
    let after: string | null = null;
    await askAssistant("helsinki", "city", { message: "hello", history: [] }, (name) => events.push(name), undefined, (retry) => (after = retry));
    expect(sent[0].headers.get("x-csrf-token")).toBe("tok en");
    expect(events).toContain("token");
    expect(after).toBe("7");
  });

  it("names the status when the refusal body is not JSON", async () => {
    answer(() => new Response("<html>", { status: 502 }));
    await expect(askAssistant("helsinki", "city", { message: "x", history: [] }, () => undefined)).rejects.toMatchObject({ status: 502, message: expect.stringContaining("502") });
  });
});

describe("tables and charts", () => {
  it("reads no table from an empty or missing column list, and shows null as nothing", () => {
    expect(dataTable({ columns: [] })).toBeNull();
    expect(dataTable({ rows: [[1]] })).toBeNull();
    expect(cellText(null)).toBe("");
    expect(cellText(undefined)).toBe("");
    expect(cellText({ a: 1 })).toBe('{"a":1}');
  });

  it("draws no chart when no column names the numbers", () => {
    expect(dataChart({ caption: "", columns: ["a", "b"], rows: [[1, 2], [3, 4]] })).toBeNull();
    expect(dataChart({ caption: "", columns: ["v"], rows: [[1], [2]] })).toBeNull();
  });
});

describe("visualOf refusals", () => {
  it("refuses a spec nested past eight levels, a list over eight entries, a spec that is a list", () => {
    let deep: unknown = 1;
    for (let i = 0; i < 9; i += 1) deep = [deep];
    expect(visualOf({ spec: { ...GOOD, layer: deep }, rows: ROWS })).toBeNull();
    expect(visualOf({ spec: { ...GOOD, layer: [1, 2, 3, 4, 5, 6, 7, 8, 9] }, rows: ROWS })).toBeNull();
    expect(visualOf({ spec: [GOOD], rows: ROWS })).toBeNull();
    expect(visualOf({ rows: ROWS })).toBeNull();
  });

  it("refuses a spec over data it does not name, or without rows", () => {
    expect(visualOf({ spec: { ...GOOD, data: { name: "other" } }, rows: ROWS })).toBeNull();
    expect(visualOf({ spec: { ...GOOD, data: { name: "source", format: {} } }, rows: ROWS })).toBeNull();
    expect(visualOf({ spec: GOOD })).toBeNull();
    expect(visualOf({ spec: GOOD, rows: ROWS })?.rows).toEqual(ROWS);
  });
});

describe("linkHostsOf", () => {
  const row = (source: string, startUrls: string[]): KnowledgeSourceRow => ({ source, type: "website", state: "crawled", startUrls, ckanInstanceRef: null, schedule: null, visibility: "public" });
  it("takes no origin from a value that is not a list and skips an address that is not a URL", () => {
    expect(linkHostsOf("https://a.example", undefined, [])).toEqual(["data.gov.sk"]);
    expect(linkHostsOf(["https://www.city.example/x", 7, "not a url"], ["web"], [row("web", ["https://news.example"]), row("other", ["https://x.example"])])).toEqual([
      "city.example",
      "data.gov.sk",
      "news.example",
    ]);
  });
});
