import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import ratchet from "./odata-ratchet.json";

// T-6812 (ADR-N-070, API/13 §2a): the Portal UI moves from the REST doors onto the Organization's
// OData service, page group by page group. Two counts may only fall, and each slice lowers the file
// beside this test by what it moved: the distinct `/api/v1/...` REST paths written in ui/src (the
// OData service's own addresses are built by the SDK client from its root, so they hold none), and
// the files that send a request with `fetch` themselves instead of through one client.

const SRC = join(__dirname, "../src");

function sources(): [string, string][] {
  return readdirSync(SRC, { recursive: true, encoding: "utf8" })
    .filter((entry) => /\.(ts|tsx)$/.test(entry))
    .sort()
    .map((entry) => [entry.split("\\").join("/"), readFileSync(join(SRC, entry), "utf8")]);
}

/** Every distinct REST path the UI's source names; the per-project OData compat alias is no REST door. */
export function restPaths(files: [string, string][]): string[] {
  const found = new Set<string>();
  for (const [, text] of files) {
    for (const [path] of text.matchAll(/\/api\/v1\/[^'"` ]+/g)) {
      if (!path.includes("/odata")) found.add(path);
    }
  }
  return [...found].sort();
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

  it("sends a request with fetch only from the files the ratchet still lists", () => {
    const calling = fetchFiles(files);
    const added = calling.filter((name) => !ratchet.fetchFiles.includes(name));
    const gone = ratchet.fetchFiles.filter((name) => !calling.includes(name));
    expect(added, "a new file calls fetch itself: send it through api/client.ts or api/odata.ts").toEqual([]);
    expect(gone, "these files no longer call fetch: remove them from fetchFiles in tests/odata-ratchet.json").toEqual([]);
  });
});
