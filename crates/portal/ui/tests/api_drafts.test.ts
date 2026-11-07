/**
 * The draft calls go through the typed client (UI-07, CC-76).
 *
 * A raw `fetch` skipped the client's middleware, so inside a workspace the form read and wrote
 * the project's own draft instead of the copy's, and an ended session answered the form with an
 * error instead of the login.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getDraft, putDraft } from "../src/api/drafts";
import { setActiveWorkspace } from "../src/components/layout/WorkspaceContext";

const draft = {
  project: "helsinki",
  kind: "Endpoint",
  name: "bikes",
  manifest: { kind: "Endpoint" },
  touchedBy: "demo.steward",
  touchedKind: "person",
  version: 3,
  updatedAt: "2026-09-21T06:00:00Z",
};

let seen: Request[] = [];

function answer(status: number, body: unknown): void {
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

beforeEach(() => {
  seen = [];
});

afterEach(() => {
  setActiveWorkspace(null);
  vi.unstubAllGlobals();
});

describe("drafts through the typed client", () => {
  // UI-07, CC-76: a copy's draft is not the project's.
  it("reads and writes the draft of the workspace the person is in", async () => {
    setActiveWorkspace("air-v2");
    answer(200, draft);
    await getDraft("helsinki", "Endpoint", "bikes");
    await putDraft("helsinki", "Endpoint", "bikes", { kind: "Endpoint" }, 3);
    expect(seen.map((r) => new URL(r.url).searchParams.get("workspace"))).toEqual([
      "air-v2",
      "air-v2",
    ]);
    expect(seen[1].method).toBe("PUT");
    expect(await seen[1].json()).toEqual({ manifest: { kind: "Endpoint" }, expectedVersion: 3 });
  });

  it("reads the project's own draft outside a workspace", async () => {
    answer(200, draft);
    expect(await getDraft("helsinki", "Endpoint", "bikes")).toMatchObject({ version: 3 });
    expect(new URL(seen[0].url).pathname).toBe("/api/v1/projects/helsinki/drafts/Endpoint/bikes");
    expect(new URL(seen[0].url).search).toBe("");
  });

  it("answers null for no draft and for an answer that is not one", async () => {
    answer(404, { detail: "no draft" });
    expect(await getDraft("helsinki", "Endpoint", "bikes")).toBeNull();
    answer(200, { something: "else" });
    expect(await getDraft("helsinki", "Endpoint", "bikes")).toBeNull();
  });

  it("refuses, never answers no draft, when the read fails (F-UI1-013)", async () => {
    // A null here lets the form write with no expectedVersion over another window's draft.
    answer(500, { detail: "the forge did not answer" });
    await expect(getDraft("helsinki", "Endpoint", "bikes")).rejects.toMatchObject({ status: 500, detail: "the forge did not answer" });
    answer(403, { detail: "no read" });
    await expect(getDraft("helsinki", "Endpoint", "bikes")).rejects.toMatchObject({ status: 403 });
  });

  it("names the version it lost to on a conflict", async () => {
    answer(409, { type: "https://joinedcontext.com/errors/draft-conflict", current: 7 });
    await expect(putDraft("helsinki", "Endpoint", "bikes", {}, 3)).rejects.toMatchObject({
      status: 409,
      current: 7,
    });
  });

  it("says why a write was refused", async () => {
    answer(422, { detail: "literal secret in field 'spec.password' is forbidden" });
    await expect(putDraft("helsinki", "Endpoint", "bikes", {})).rejects.toThrow(
      "failed to put draft (422): literal secret in field 'spec.password' is forbidden",
    );
  });

  // MF-24, T-2626: the refusal keeps its status and the server's sentence, so the dialog can say it.
  it("keeps the status and the server's sentence of a refused write", async () => {
    answer(400, { detail: "literal secret in field 'password' is forbidden; use secretRef instead (MF-24)" });
    await expect(putDraft("helsinki", "DataSource", "feed", {})).rejects.toMatchObject({
      status: 400,
      detail: "literal secret in field 'password' is forbidden; use secretRef instead (MF-24)",
    });
  });

  it("carries the double-submit token on the write", async () => {
    document.cookie = "__Host-jc_csrf=t0k; Secure; path=/";
    answer(200, draft);
    await putDraft("helsinki", "Endpoint", "bikes", {});
    expect(seen[0].headers.get("x-csrf-token")).toBe("t0k");
    document.cookie = "__Host-jc_csrf=; expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; path=/";
  });
});
