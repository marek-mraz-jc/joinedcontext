/**
 * A model's LinkML source through the typed client (UI-07, DM-56, DM-57).
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { readModelSource, writeModelSource } from "../src/api/datamodelSource";

const YAML = "id: https://example.org/air\nname: air\nclasses: {}\n";
let seen: Request[] = [];

function answer(status: number, body: string, type = "application/json"): void {
  seen = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      seen.push(request);
      return new Response(body, { status, headers: { "content-type": type } });
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the model source (UI-07)", () => {
  it("reads the committed YAML as text", async () => {
    answer(200, YAML, "text/yaml");
    expect(await readModelSource("helsinki", "air")).toBe(YAML);
    expect(new URL(seen[0].url).pathname).toBe("/api/v1/projects/helsinki/datamodels/air/source");
  });

  it("names the status when the source cannot be read", async () => {
    answer(404, JSON.stringify({ title: "Not Found", status: 404 }));
    await expect(readModelSource("helsinki", "air")).rejects.toThrow("HTTP 404");
  });

  // UI-07: the body is the YAML as typed, sent as text/yaml, with the csrf token the client adds.
  it("checks the YAML as typed, not as a JSON string", async () => {
    document.cookie = "__Host-jc_csrf=t0k; Secure; path=/";
    answer(200, JSON.stringify({ severity: "minor", version: "1.1.0", changes: [], artifacts: {} }));
    const verdict = await writeModelSource({ project: "helsinki", name: "air", source: YAML, dryRun: true });
    expect(verdict).toMatchObject({ kind: "checked", result: { severity: "minor", version: "1.1.0" } });
    const request = seen[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).search).toBe("?dryRun=All");
    expect(request.headers.get("content-type")).toBe("text/yaml; charset=utf-8");
    expect(request.headers.get("x-csrf-token")).toBe("t0k");
    expect(await request.text()).toBe(YAML);
    document.cookie = "__Host-jc_csrf=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/";
  });

  it("proposes a new model in the space it names, and only then sends the space", async () => {
    answer(202, JSON.stringify({ kind: "Change", metadata: { name: "mr-9" } }));
    const proposed = await writeModelSource({
      project: "helsinki",
      name: "bikes",
      source: YAML,
      dryRun: false,
      space: "mobility",
    });
    expect(proposed).toMatchObject({ kind: "proposed", change: { metadata: { name: "mr-9" } } });
    expect(new URL(seen[0].url).search).toBe("?space=mobility");
  });

  it("hands back the refusal and its reasons", async () => {
    answer(400, JSON.stringify({ title: "Bad Request", status: 400, errors: ["slot 'x' has no range"] }));
    const refused = await writeModelSource({ project: "helsinki", name: "air", source: YAML, dryRun: true });
    expect(refused).toEqual({
      kind: "refused",
      status: 400,
      problem: { title: "Bad Request", status: 400, errors: ["slot 'x' has no range"] },
    });
  });

  it("does not take a check's 200 for a proposal", async () => {
    answer(200, JSON.stringify({ severity: "minor", version: "1.1.0", changes: [], artifacts: {} }));
    const answered = await writeModelSource({ project: "helsinki", name: "air", source: YAML, dryRun: false });
    expect(answered.kind).toBe("refused");
  });
});
