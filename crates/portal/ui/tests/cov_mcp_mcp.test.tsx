/**
 * T-7589: coverage for src/pages/mcp/mcp.ts.
 * Exercises asChoice slug validation and defaults, english title/description unwrapping,
 * allowedProjects filtering, probe error paths and AbortError, and the readCard RPC gateway reader.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Manifest } from "../src/api/manifest";
import { asChoice, breadth, emptyForm, formProblems, fromManifest, probe, readCard, rpcAnswer, toManifest, toolsOf, widestAllowed, type MemberChoice } from "../src/pages/mcp/mcp";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("asChoice edge cases", () => {
  it("returns null when slug is empty, non-string, or missing (line 38)", () => {
    const base: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "ep", namespace: "city" },
      spec: {},
    };
    expect(asChoice("city", { ...base, spec: { slug: "" } }, "Title")).toBeNull();
    expect(asChoice("city", { ...base, spec: { slug: 123 } }, "Title")).toBeNull();
    expect(asChoice("city", base, "Title")).toBeNull();
  });

  it("falls back to project-list audience when audience is unrecognized", () => {
    const ep: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "ep", namespace: "city" },
      spec: { slug: "valid-slug", audience: "unrecognized-scope" },
    };
    const choice = asChoice("city", ep, "Title");
    expect(choice?.audience).toBe("project-list");
    expect(choice?.servesMcp).toBe(true);
  });
});

describe("fromManifest and english extraction", () => {
  it("extracts plain string or localized en strings, and returns empty string for other values (lines 109, 114)", () => {
    const stringManifest: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "McpServer",
      metadata: { name: "srv", namespace: "city", title: "Direct String Title", description: "Direct String Desc" },
      spec: {},
    };
    const form1 = fromManifest("city", stringManifest);
    expect(form1.title).toBe("Direct String Title");
    expect(form1.description).toBe("Direct String Desc");

    const nonEnManifest: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "McpServer",
      metadata: { name: "srv", namespace: "city", title: { sk: "Titulok" }, description: { de: "Beschreibung" } },
      spec: {},
    };
    const form2 = fromManifest("city", nonEnManifest);
    expect(form2.title).toBe("");
    expect(form2.description).toBe("");

    const otherTypesManifest: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "McpServer",
      metadata: { name: "srv", namespace: "city", title: 42 as unknown as string, description: null as unknown as string },
      spec: {},
    };
    const form3 = fromManifest("city", otherTypesManifest);
    expect(form3.title).toBe("");
    expect(form3.description).toBe("");
  });

  it("filters non-string entries in allowedProjects (line 135)", () => {
    const manifest: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "McpServer",
      metadata: { name: "srv", namespace: "city" },
      spec: {
        allowedProjects: ["proj-1", 1234, null, "proj-2"],
        members: [{ name: "ep1" }, { name: "ep2", namespace: "other" }, { noname: true }],
        select: { tags: ["public"] },
      },
    };
    const form = fromManifest("city", manifest);
    expect(form.allowedProjects).toEqual(["proj-1", "proj-2"]);
    expect(form.members).toEqual(["city/ep1", "other/ep2"]);
    expect(form.select).toEqual({ tags: ["public"] });
  });
});

describe("probe HTTP and RPC error paths", () => {
  it("handles 401 and 404 as refused states (line 217)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 401 })));
    expect(await probe("slug-401")).toEqual({ state: "refused", status: 401 });

    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 404 })));
    expect(await probe("slug-404")).toEqual({ state: "refused", status: 404 });
  });

  it("handles non-ok 500 response as failed with HTTP status (line 219)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("Server error", { status: 500 })));
    expect(await probe("slug-500")).toEqual({ state: "failed", reason: "HTTP 500" });
  });

  it("handles JSON-RPC error response with string and non-string messages (line 222)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { message: "Custom RPC failure" } }), { status: 200 })));
    expect(await probe("slug-rpc-str")).toEqual({ state: "failed", reason: "Custom RPC failure" });

    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { message: { complex: true } } }), { status: 200 })));
    expect(await probe("slug-rpc-obj")).toEqual({ state: "failed", reason: "error" });
  });

  it("re-throws AbortError on probe abort (line 225)", async () => {
    const abort = new Error("aborted");
    abort.name = "AbortError";
    vi.stubGlobal("fetch", vi.fn(async () => { throw abort; }));
    await expect(probe("slug-abort")).rejects.toThrow("aborted");
  });

  it("handles generic Error and non-Error throwables (line 226)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => { throw new Error("Connection failed"); }));
    expect(await probe("slug-err")).toEqual({ state: "failed", reason: "Connection failed" });

    vi.stubGlobal("fetch", vi.fn(async () => { throw "non-error-thrown"; }));
    expect(await probe("slug-string")).toEqual({ state: "failed", reason: "unreachable" });
  });
});

describe("readCard RPC resource reader", () => {
  it("reads card contents from successful response (lines 256-273)", async () => {
    const body = JSON.stringify({ result: { contents: [{ text: "## NGSI-LD Entity Card" }] } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(body, { status: 200, headers: { "Content-Type": "application/json" } })));
    const res = await readCard("ep-slug");
    expect(res).toEqual({ state: "card", text: "## NGSI-LD Entity Card" });
  });

  it("reads card contents from event-stream response", async () => {
    const sse = `event: message\ndata: ${JSON.stringify({ result: { contents: [{ text: "Streamed Card" }] } })}\n\n`;
    vi.stubGlobal("fetch", vi.fn(async () => new Response(sse, { status: 200, headers: { "Content-Type": "text/event-stream" } })));
    const res = await readCard("ep-stream-slug");
    expect(res).toEqual({ state: "card", text: "Streamed Card" });
  });

  it("handles 401, 403, and 404 as refused status", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 401 })));
    expect(await readCard("slug-401")).toEqual({ state: "refused", status: 401 });

    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 403 })));
    expect(await readCard("slug-403")).toEqual({ state: "refused", status: 403 });

    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 404 })));
    expect(await readCard("slug-404")).toEqual({ state: "refused", status: 404 });
  });

  it("throws on non-ok HTTP statuses", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("Gateway error", { status: 502 })));
    await expect(readCard("slug-502")).rejects.toThrow("HTTP 502");
  });

  it("returns refused status 403 when JSON-RPC body contains error", async () => {
    const body = JSON.stringify({ error: { code: -32603, message: "Denied" } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(body, { status: 200 })));
    expect(await readCard("slug-rpc-err")).toEqual({ state: "refused", status: 403 });
  });

  it("throws when result.isError is true or contents text is not a string", async () => {
    const isErrorBody = JSON.stringify({ result: { isError: true } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(isErrorBody, { status: 200 })));
    await expect(readCard("slug-is-err")).rejects.toThrow("the card could not be read");

    const missingTextBody = JSON.stringify({ result: { contents: [] } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(missingTextBody, { status: 200 })));
    await expect(readCard("slug-no-text")).rejects.toThrow("the card could not be read");

    const numberTextBody = JSON.stringify({ result: { contents: [{ text: 999 }] } });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(numberTextBody, { status: 200 })));
    await expect(readCard("slug-num-text")).rejects.toThrow("the card could not be read");
  });
});

describe("toManifest, formProblems, rpcAnswer, and toolsOf helpers", () => {
  it("builds manifest with title, description, and allowedProjects when applicable", () => {
    const form = {
      ...emptyForm(),
      name: "mobility",
      title: "Mobility API",
      description: "Urban sensors",
      audience: "project-list" as const,
      allowedProjects: ["doprava"],
      members: ["city/bikes"],
      knowledgeSources: ["portal-docs"],
      select: { domain: "traffic" },
    };
    const manifest = toManifest("city", form);
    expect((manifest.metadata as Record<string, unknown>).title).toEqual({ en: "Mobility API" });
    expect((manifest.metadata as Record<string, unknown>).description).toEqual({ en: "Urban sensors" });
    expect((manifest.spec as Record<string, unknown>).allowedProjects).toEqual(["doprava"]);
    expect((manifest.spec as Record<string, unknown>).knowledgeSources).toEqual(["portal-docs"]);
    expect((manifest.spec as Record<string, unknown>).select).toEqual({ domain: "traffic" });
  });

  it("validates breadth, widestAllowed, and formProblems", () => {
    expect(breadth(undefined)).toBe(0);
    expect(breadth("public")).toBe(2);
    expect(breadth("organization")).toBe(1);
    expect(breadth("project-list")).toBe(0);

    expect(widestAllowed([])).toBe("project-list");

    const memberChoice: MemberChoice = { project: "city", name: "m1", title: "M1", slug: "s1", audience: "public", servesMcp: true };
    expect(widestAllowed([memberChoice])).toBe("public");

    const validForm = { ...emptyForm(), name: "valid-name", members: ["city/m1"] };
    expect(formProblems(validForm, [memberChoice])).toEqual({});

    const badProjForm = { ...validForm, audience: "project-list" as const, allowedProjects: ["INVALID PROJ"] };
    expect(formProblems(badProjForm, [memberChoice]).allowedProjects).toBe("mcp.problem.projects");
  });

  it("handles rpcAnswer with empty text-event-stream and toolsOf edge cases", () => {
    expect(rpcAnswer("text/event-stream", "comment: only\n\n")).toBeNull();
    expect(toolsOf(null)).toEqual([]);
    expect(toolsOf({ result: { tools: [{ name: "query", description: null }] } })).toEqual([
      { name: "query", description: "", readOnly: false },
    ]);
  });
});
