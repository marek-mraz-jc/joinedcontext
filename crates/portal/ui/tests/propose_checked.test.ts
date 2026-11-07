import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import { proposeBundle, proposeChecked } from "../src/api/proposal";

/** Every request the helper sends: method, path, whether it was the dry run, and its body. */
function stubFetch(checkAnswer: unknown) {
  const sent: { method: string; path: string; dryRun: boolean; body: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = typeof input === "string" || input instanceof URL ? null : input;
      const url = new URL(request ? request.url : String(input), window.location.origin);
      const dryRun = url.searchParams.get("dryRun") === "All";
      sent.push({
        method: request?.method ?? init?.method ?? "GET",
        path: url.pathname,
        dryRun,
        body: request ? await request.clone().text() : String(init?.body ?? ""),
      });
      const json = (body: unknown, status: number) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      return dryRun
        ? json(checkAnswer, 200)
        : json({ kind: "Change", metadata: { name: "chg-00000001" } }, 202);
    }),
  );
  return sent;
}

const SPACE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "air" }, spec: {} };

afterEach(() => vi.unstubAllGlobals());

describe("proposeChecked (PF-57, T-0956)", () => {
  it("checks the manifest on the same route, then proposes the same body", async () => {
    const sent = stubFetch({ valid: true, verdict: { ok: true, findings: [] } });
    const change = await proposeChecked("helsinki", "spaces", SPACE, true);

    expect(change).toMatchObject({ kind: "Change" });
    expect(sent.map(({ method, path, dryRun }) => [method, path, dryRun])).toEqual([
      ["POST", "/api/v1/projects/helsinki/spaces", true],
      ["POST", "/api/v1/projects/helsinki/spaces", false],
    ]);
    expect(sent[0].body).toBe(sent[1].body);
  });

  it("checks and proposes an update on the resource's own route", async () => {
    const sent = stubFetch({ valid: true });
    await proposeChecked("helsinki", "spaces", SPACE, false);

    expect(sent.map(({ method, path, dryRun }) => [method, path, dryRun])).toEqual([
      ["PUT", "/api/v1/projects/helsinki/spaces/air", true],
      ["PUT", "/api/v1/projects/helsinki/spaces/air", false],
    ]);
  });

  it("proposes nothing after a red check and says what the check found", async () => {
    const sent = stubFetch({
      valid: false,
      verdict: { ok: false, findings: [{ level: "error", path: "spec.dataModelRef", message: "names no model" }] },
    });

    const refused = proposeChecked("helsinki", "spaces", SPACE, true);
    await expect(refused).rejects.toBeInstanceOf(ApiError);
    await expect(refused).rejects.toThrow("names no model");
    expect(sent).toHaveLength(1);
    expect(sent[0].dryRun).toBe(true);
  });

  it("refuses a red verdict even when the plan itself was valid", async () => {
    const sent = stubFetch({ valid: true, verdict: { ok: false, findings: [] } });

    await expect(proposeChecked("helsinki", "spaces", SPACE, true)).rejects.toThrow(
      "the check found problems",
    );
    expect(sent).toHaveLength(1);
  });
});

describe("proposeBundle (PF-57, F-UI2-013)", () => {
  const DRAWN = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ModelProjection", metadata: { name: "air-filter" }, spec: {} };
  const TAKEN = "ModelProjection 'air-filter' already exists in project 'helsinki'; choose skip, replace or rename (MF-23)";

  it("checks and proposes the first projection refusing a name someone else holds", async () => {
    const sent = stubFetch({ created: ["air-filter"] });
    await proposeBundle("helsinki", [DRAWN]);

    expect(sent.map(({ path, dryRun }) => [path, dryRun])).toEqual([
      ["/api/v1/projects/helsinki/import", true],
      ["/api/v1/projects/helsinki/import", false],
    ]);
    for (const { body } of sent) {
      expect(JSON.parse(body).conflictPolicy).toBe("fail");
    }
  });

  it("proposes nothing when the check finds the name taken and carries the import's detail", async () => {
    const sent: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        sent.push(String(input instanceof Request ? input.url : input));
        return new Response(JSON.stringify({ type: "about:blank", title: "Conflict", status: 409, detail: TAKEN }), {
          status: 409,
          headers: { "Content-Type": "application/problem+json" },
        });
      }),
    );

    const refused = proposeBundle("helsinki", [DRAWN]);
    await expect(refused).rejects.toBeInstanceOf(ApiError);
    await expect(refused).rejects.toMatchObject({ status: 409, problem: { detail: expect.stringContaining("'air-filter' already exists") } });
    expect(sent).toHaveLength(1);
  });
});
