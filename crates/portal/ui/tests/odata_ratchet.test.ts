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

/** A path with each `{name}` and `${expression}` segment written `{}`, and no query. */
export function shape(path: string): string {
  return path
    .replace(/\$\{[^}]*\}?/g, "{}")
    .replace(/\{[^}]*\}/g, "{}")
    .replace(/\?.*$/, "");
}

/**
 * The REST addresses the UI keeps (API/13 §2b): no OData operation can carry them, so they are not
 * counted. Each entry names the class and its shapes; a shape no source writes any more fails the
 * last test, so the list only ever names what is in use.
 */
export const STAY_REST: { why: string; shapes: string[] }[] = [
  {
    why: "OIDC sign-in, sign-out, the session probe and the Apps' single sign-on check",
    shapes: [
      "/api/v1/auth/login",
      "/api/v1/auth/logout",
      "/api/v1/auth/me",
      "/api/v1/auth/sso-check",
      "/api/v1/auth/sso-check/done",
    ],
  },
  {
    why: "branding, read before sign-in; the logo and favicon are images loaded by their address",
    shapes: ["/api/v1/branding", "/api/v1/branding/logo", "/api/v1/branding/favicon"],
  },
  {
    why: "the public catalogue, read by a visitor with no session, which the service does not serve",
    shapes: [
      "/api/v1/catalogue",
      "/api/v1/catalogue/readable",
      "/api/v1/catalogue/datasets/{}",
      "/api/v1/catalogue/datasets/{}/map",
      "/api/v1/catalogue/endpoints/{}/standards",
    ],
  },
  {
    why: "an image or a map style the browser or MapLibre loads by its address",
    shapes: ["/api/v1/app-templates/{}/screenshot/1440", "/api/v1/projects/{}/basemap/default/style.json"],
  },
  {
    why: "an App preview's calls of its own functions, which the App's code addresses by path and query, relayed as it sent them (as its Endpoint calls go to /api/endpoint/)",
    shapes: ["/api/v1/projects/{}/agent-runs/{}/functions/"],
  },
  {
    why: "server-sent event streams: a run's, the drafts', the activity feed's, a paired tab's, a space's live view, an assistant's answer",
    shapes: [
      "/api/v1/projects/{}/agent-runs/{}/events",
      "/api/v1/projects/{}/drafts/events",
      "/api/v1/projects/{}/activity/stream{}",
      "/api/v1/connectors/{}/tab/{}/events",
      "/api/v1/projects/{}/spaces/{}/live",
      "/api/v1/projects/{}/knowledge/deployments/{}/chat",
    ],
  },
  {
    why: "file downloads: CSV exports and archives saved as files",
    shapes: [
      "/api/v1/projects/{}/changes/history/export",
      "/api/v1/projects/{}/pipelines/{}/rejected/export",
      "/api/v1/projects/{}/export",
      "/api/v1/projects/{}/apps/{}/export",
      "/api/v1/projects/{}/usage/endpoints/export",
      "/api/v1/projects/{}/knowledge/conversations/export",
      "/api/v1/organization/costs/export",
    ],
  },
  {
    why: "uploads that send a file's bytes: a bundle or archive import, a knowledge source's file, a sample a model is inferred from, an open data portal's file",
    shapes: [
      "/api/v1/projects/{}/import",
      "/api/v1/projects/{}/knowledge/sources/{}/files",
      "/api/v1/tools/infer-schema",
      "/api/v1/projects/org/portals/{}/files",
      "/api/v1/projects/org/portals/{}/files/{}",
    ],
  },
];

const KEPT = new Set(STAY_REST.flatMap(({ shapes }) => shapes));

/** Every REST address the UI writes, once per place, kept ones included; the per-project OData alias is no REST door. */
export function writtenPaths(files: [string, string][]): string[] {
  return files.flatMap(([, text]) =>
    [...text.matchAll(/\/api\/v1\/[^'"` ]+/g)].map(([path]) => path).filter((path) => !path.includes("/odata")),
  );
}

/** Every REST path the UI's source writes, once per place, but the kept ones of `STAY_REST`. */
export function restCalls(files: [string, string][]): string[] {
  return writtenPaths(files).filter((path) => !KEPT.has(shape(path)));
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

  it("keeps only the REST addresses API/13 §2b names, and each kept shape is still written", () => {
    expect(shape("/api/v1/projects/${encodeURIComponent(project)}/spaces/${encodeURIComponent(space)}/live?type=x")).toBe(
      "/api/v1/projects/{}/spaces/{}/live",
    );
    expect(restCalls([["a.ts", 'api.GET("/api/v1/auth/me"); api.GET("/api/v1/auth/people");']])).toEqual([
      "/api/v1/auth/people",
    ]);
    const written = new Set(writtenPaths(files).map(shape));
    const unused = [...KEPT].filter((kept) => !written.has(kept));
    expect(unused, "no source writes these kept addresses any more: remove them from STAY_REST").toEqual([]);
  });

  it("sends a request with fetch only from the files the ratchet still lists", () => {
    const calling = fetchFiles(files);
    const added = calling.filter((name) => !ratchet.fetchFiles.includes(name));
    const gone = ratchet.fetchFiles.filter((name) => !calling.includes(name));
    expect(added, "a new file calls fetch itself: send it through api/client.ts or api/odata.ts").toEqual([]);
    expect(gone, "these files no longer call fetch: remove them from fetchFiles in tests/odata-ratchet.json").toEqual([]);
  });
});
