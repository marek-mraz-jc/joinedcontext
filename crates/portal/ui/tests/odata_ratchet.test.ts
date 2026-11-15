import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import ratchet from "./odata-ratchet.json";

// T-6812 (ADR-N-070, API/13 §2a): the Portal UI moves from the REST doors onto the Organization's
// OData service, page group by page group. Two counts may only fall, and each slice lowers the file
// beside this test by what it moved: the distinct `/api/v1/...` REST paths written in ui/src and the
// places that write one (the OData service's own addresses are built by the SDK client from its
// root, so they hold none), and the files that send a request with `fetch` themselves instead of
// through one client. The REST types generated from openapi.json (api/schema.d.ts) name every
// path the server has, so they are not counted; T-7075 deletes them when the count reaches 0.

const SRC = join(__dirname, "../src");

function sources(): [string, string][] {
  return readdirSync(SRC, { recursive: true, encoding: "utf8" })
    .filter((entry) => /\.(ts|tsx)$/.test(entry) && !entry.endsWith("schema.d.ts"))
    .sort()
    .map((entry) => [entry.split("\\").join("/"), readFileSync(join(SRC, entry), "utf8")]);
}

/** Every REST path the UI's source writes, once per place; the per-project OData alias is no REST door. */
export function restCalls(files: [string, string][]): string[] {
  return files.flatMap(([, text]) =>
    [...text.matchAll(/\/api\/v1\/[^'"` ]+/g)].map(([path]) => path).filter((path) => !path.includes("/odata")),
  );
}

/** Every distinct REST path the UI's source names. */
export function restPaths(files: [string, string][]): string[] {
  return [...new Set(restCalls(files))].sort();
}

/** The files that call `fetch` (or `window.fetch`, `globalThis.fetch`) themselves. */
export function fetchFiles(files: [string, string][]): string[] {
  return files.filter(([, text]) => /(?<![.\w])fetch\(|\b(?:window|globalThis)\.fetch\(/.test(text)).map(([name]) => name);
}

describe("the OData ratchet of the Portal UI", () => {
  const files = sources();

  it("counts a path and a fetch as the gate means them", () => {
    const sample: [string, string][] = [
      ["a.ts", 'api.GET("/api/v1/projects/{project}/secrets"); fetch(`/api/v1/projects/${p}/odata/x`);'],
      ["b.ts", "client.fetchQuery({}); await window.fetch(url);"],
      ["c.ts", "queryClient.prefetch(); portal.from(\"endpoints\").page();"],
    ];
    expect(restPaths(sample)).toEqual(["/api/v1/projects/{project}/secrets"]);
    expect(restCalls([...sample, ["d.ts", 'api.GET("/api/v1/projects/{project}/secrets")']])).toHaveLength(2);
    expect(fetchFiles(sample)).toEqual(["a.ts", "b.ts"]);
  });

  it("names no more REST paths than the ratchet allows, and the ratchet follows every one removed", () => {
    const count = restPaths(files).length;
    expect(
      count,
      count > ratchet.restPaths
        ? "a new /api/v1 REST path in ui/src: read through the OData service (ui/src/api/odata.ts) instead"
        : `the UI names ${count} REST paths now: lower restPaths in tests/odata-ratchet.json to ${count}`,
    ).toBe(ratchet.restPaths);
  });

  it("writes no more REST calls than the ratchet allows, and the ratchet follows every one removed", () => {
    const count = restCalls(files).length;
    expect(
      count,
      count > ratchet.restCalls
        ? "a new /api/v1 REST call in ui/src: read through the OData service (ui/src/api/odata.ts) instead"
        : `the UI writes ${count} REST calls now: lower restCalls in tests/odata-ratchet.json to ${count}`,
    ).toBe(ratchet.restCalls);
  });

  it("sends a request with fetch only from the files the ratchet still lists", () => {
    const calling = fetchFiles(files);
    const added = calling.filter((name) => !ratchet.fetchFiles.includes(name));
    const gone = ratchet.fetchFiles.filter((name) => !calling.includes(name));
    expect(added, "a new file calls fetch itself: send it through api/client.ts or api/odata.ts").toEqual([]);
    expect(gone, "these files no longer call fetch: remove them from fetchFiles in tests/odata-ratchet.json").toEqual([]);
  });
});
