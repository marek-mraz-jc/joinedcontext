/**
 * A project operation and a pipeline test, through the typed client (UI-07, PL-43).
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { callOperation } from "../src/api/operations";
import { testPipeline } from "../src/api/pipelineTest";

let seen: Request[] = [];

function answer(status: number, body: unknown): void {
  seen = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      seen.push(request);
      return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      });
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("callOperation", () => {
  it("posts the input to the named operation with the csrf token and hands back its output", async () => {
    document.cookie = "__Host-jc_csrf=t0k; Secure; path=/";
    answer(200, { spaces: ["air"] });
    const answered = await callOperation("helsinki", "jc_space_complete", { propose: false });
    expect(answered).toEqual({ ok: true, status: 200, output: { spaces: ["air"] } });
    expect(new URL(seen[0].url).pathname).toBe("/api/v1/projects/helsinki/ops/jc_space_complete");
    expect(seen[0].headers.get("x-csrf-token")).toBe("t0k");
    expect(await seen[0].json()).toEqual({ propose: false });
    document.cookie = "__Host-jc_csrf=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/";
  });

  it("gives the operation's own reason when it refuses", async () => {
    answer(422, { title: "Unprocessable", status: 422, detail: "the url answered 404" });
    expect(await callOperation("helsinki", "jc_space_complete", {})).toEqual({
      ok: false,
      status: 422,
      reason: "the url answered 404",
    });
  });

  it("falls back to the error code, then to nothing, never to an empty sentence", async () => {
    answer(409, { error: "draft_conflict", detail: "" });
    expect(await callOperation("helsinki", "x", {})).toMatchObject({ reason: "draft_conflict" });
    answer(500, {});
    expect(await callOperation("helsinki", "x", {})).toEqual({ ok: false, status: 500, reason: undefined });
  });
});

describe("testPipeline", () => {
  const trace = { input: { events: 1, bytes: 10 }, mapping: [], validation: [], errors: [] };

  it("sends the candidate and the sample and reads the trace", async () => {
    answer(200, trace);
    const answered = await testPipeline("helsinki", { kind: "Pipeline" }, { text: "a,b\n1,2", format: "csv" });
    expect(answered).toEqual({ trace });
    expect(new URL(seen[0].url).pathname).toBe("/api/v1/projects/helsinki/pipelines/test");
    expect(await seen[0].json()).toEqual({
      pipeline: { kind: "Pipeline" },
      sample: { text: "a,b\n1,2", format: "csv" },
    });
  });

  it("names the status and the reason of a refused run", async () => {
    answer(409, { title: "Conflict", status: 409, detail: "A test of this project is already running" });
    expect(await testPipeline("helsinki", {}, { url: "https://example.org/a.json", format: "json" })).toEqual({
      status: 409,
      detail: "A test of this project is already running",
    });
  });
});
