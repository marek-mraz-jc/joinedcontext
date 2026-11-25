// T-7706: the OData client's edges: a removal proposed inside a workspace's copy, a removal of a set the service does not have, and the merge requests outside the Changes of a project.
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import { listOutside, proposeDelete } from "../src/api/odata";
import { activeWorkspace } from "../src/components/layout/WorkspaceContext";
import { odataValue } from "./odata";

vi.mock("../src/components/layout/WorkspaceContext", async (actual) => ({
  ...(await actual<object>()),
  activeWorkspace: vi.fn(() => null),
}));

afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(activeWorkspace).mockReturnValue(null);
});

function serve(value: unknown): Request[] {
  const sent: Request[] = [];
  // Not vi.stubGlobal: tests/setup.ts would answer these doors from their REST stubs.
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    sent.push(input as Request);
    return odataValue(value);
  });
  return sent;
}

describe("the OData client's removals", () => {
  it("proposes a removal inside the workspace's copy, naming the workspace and the check", async () => {
    const sent = serve({ kind: "Change", metadata: { name: "chg-1" } });
    vi.mocked(activeWorkspace).mockReturnValue("draft");
    await proposeDelete("air", "spaces", "pm", { dryRun: true, ifMatch: '"3f2a"' });
    expect(sent).toHaveLength(1);
    expect(new URL(sent[0].url).pathname).toContain("portal_manifest_delete");
    await expect(sent[0].json()).resolves.toMatchObject({ plural: "spaces", name: "pm", workspace: "draft", dryRun: "All", ifMatch: '"3f2a"' });
  });

  it("refuses to remove from a set the service does not have, before anything is sent", async () => {
    const sent = serve({});
    const failure = await proposeDelete("air", "nonsense", "x").catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).status).toBe(404);
    expect((failure as ApiError).message).toBe("The service has no set of manifests called nonsense.");
    expect(sent).toHaveLength(0);
  });
});

describe("the merge requests outside a project's Changes", () => {
  it("reads those of the Organization's own namespace from its change list", async () => {
    const sent = serve({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [], outside: [{ number: 9, reason: "a fork's" }] });
    await expect(listOutside("org")).resolves.toEqual([{ number: 9, reason: "a fork's" }]);
    expect(decodeURIComponent(new URL(sent[0].url).pathname)).toContain("portal_organization_change_list");
  });

  it("lists those the door names for a project that is not the Organization's", async () => {
    const sent = serve({ items: [{ number: 7, reason: "a fork's" }] });
    await expect(listOutside("air")).resolves.toEqual([{ number: 7, reason: "a fork's" }]);
    expect(decodeURIComponent(new URL(sent[0].url).pathname)).toContain("projects('air')/JoinedContext.Portal.portal_change_outside");
  });
});
