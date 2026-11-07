import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../src/sdk/client";
import type { JcConfig } from "../src/sdk/config";
import { entitiesUrl } from "../src/ngsi";
import { formEncode, queryString, randomId } from "../src/sdk/query";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

const CONFIG: JcConfig = { slug: "demo", orgDomain: "example.org", space: "demo", transport: "bridge", language: "fi" };

describe("queryString", () => {
  it.each([
    [{ q: "name==\"Kamppi (metro)\"", attrs: "a,b" }],
    [{ q: "name~=Töölö;free>0", options: "keyValues" }],
    [{ q: "a b!c'd(e)f~g*h-i.j_k", limit: "10" }],
    [{ georel: "near;maxDistance==500", coordinates: "[24.94,60.17]" }],
  ])("writes what URLSearchParams writes for %j", (params) => {
    expect(queryString(params)).toBe(new URLSearchParams(params).toString());
  });

  it("leaves an undefined value out and keeps the insertion order", () => {
    expect(queryString({ type: "T", offset: undefined, limit: "5" })).toBe("type=T&limit=5");
  });

  it("encodes a space as + and unicode as UTF-8, the same on every call", () => {
    expect(formEncode("Töölö bay")).toBe("T%C3%B6%C3%B6l%C3%B6+bay");
    expect(formEncode("Töölö bay")).toBe(formEncode("Töölö bay"));
  });

  it("makes a v4 id without crypto", () => {
    vi.stubGlobal("crypto", undefined);
    const id = randomId();
    expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(randomId()).not.toBe(id);
  });
});

// jc-functions runs QuickJS, which has no Web APIs: the client must work with none of them.
describe("the client without URLSearchParams, URL or crypto", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("builds the same list, get, temporal and create requests", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req: JcRequest): Promise<JcResponse> => {
      calls.push(req);
      if (req.method === "POST") return { status: 201, body: null };
      return { status: 200, body: req.path.includes("/entities/") ? { id: "urn:x", type: "T" } : [] };
    };
    const expectedList = `type=Stop&options=keyValues&limit=10&offset=20&attrs=name%2Cfree&q=name%3D%3D%22A+B%22`;
    const expectedTemporal = new URLSearchParams({ type: "Stop", options: "temporalValues", timerel: "after", timeAt: "2026-01-01T00:00:00Z", lastN: "3" }).toString();
    vi.stubGlobal("URLSearchParams", undefined);
    vi.stubGlobal("URL", undefined);
    vi.stubGlobal("crypto", undefined);

    const client = createClient(CONFIG, transport);
    await client.entities.list("Stop", { limit: 10, offset: 20, attrs: ["id", "name", "free"], q: 'name=="A B"' });
    await client.entities.get("urn:ngsi-ld:Stop:example.org:demo:1", ["name"]);
    await client.temporal.list("Stop", { timerel: "after", timeAt: "2026-01-01T00:00:00Z", lastN: 3 });
    const created = await client.entities.create("Stop", { name: "A" });

    expect(calls[0].path).toBe(`/api/endpoint/demo/ngsi-ld/v1/entities?${expectedList}`);
    expect(calls[1].path).toBe(
      "/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3AStop%3Aexample.org%3Ademo%3A1?options=keyValues&attrs=name",
    );
    expect(calls[2].path).toBe(`/api/endpoint/demo/ngsi-ld/v1/temporal/entities?${expectedTemporal}`);
    expect(created).toMatch(/^urn:ngsi-ld:Stop:example\.org:demo:[0-9a-f-]{36}$/);
    expect(entitiesUrl("s", { name: "s", type: "T", attrs: ["a"], q: "x>1", limit: 50 }, 0)).toBe(
      "/api/endpoint/s/ngsi-ld/v1/entities?type=T&options=keyValues&limit=50&attrs=a&q=x%3E1",
    );
  });
});

// Everything `@joinedcontext/sdk/server` bundles for jc-functions, followed from its entry.
const SOURCES = import.meta.glob("../src/**/*.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

function serverGraph(): string[] {
  const seen = new Set<string>();
  const visit = (file: string) => {
    if (seen.has(file) || SOURCES[file] === undefined) return;
    seen.add(file);
    for (const [, spec] of SOURCES[file].matchAll(/^(?:import|export)\s+(?!type\b)[^;]*?from\s+"(\.[^"]+)"/gm)) {
      const dir = file.slice(0, file.lastIndexOf("/"));
      const parts = `${dir}/${spec}`.split("/");
      const path: string[] = [];
      for (const part of parts) {
        if (part === ".." && path.length > 0 && path[path.length - 1] !== "..") path.pop();
        else if (part !== ".") path.push(part);
      }
      visit(`${path.join("/")}.ts`);
    }
  };
  visit("../src/sdk/functions-runtime.ts");
  return [...seen];
}

describe("the server bundle", () => {
  it("uses no browser Web API that QuickJS lacks", () => {
    const files = serverGraph();
    expect(files).toContain("../src/sdk/client.ts");
    const offenders = files.flatMap((file) => {
      const code = SOURCES[file].replace(/^\s*\/\/.*$/gm, "");
      return [/\bURLSearchParams\b/, /\bnew URL\(/, /\bcrypto\.randomUUID\(\)/]
        .filter((pattern) => file !== "../src/sdk/query.ts" && pattern.test(code))
        .map((pattern) => `${file}: ${pattern}`);
    });
    expect(offenders).toEqual([]);
  });
});
