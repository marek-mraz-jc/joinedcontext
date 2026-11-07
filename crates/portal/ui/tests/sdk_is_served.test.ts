/**
 * T-1777…T-1787: the linked SDK is reachable from the UI's own build (UI-01, PF-50).
 *
 * `@joinedcontext/sdk` is `link:../sdk` and its package entry is TypeScript source. Vite serves
 * such a file over `/@fs/`, guarded by `server.fs.allow`, which Vite 8 derives from the nearest
 * package root — `ui/`. `../sdk` fell outside it and 86 of the 142 test files stopped collecting
 * with "Cannot find module /@fs/…/sdk/src/…", a failure in no source file and in no test.
 * This is the one cheap test that names the cause, so the next upgrade that narrows the
 * allow-list fails here instead of in every page test at once.
 */
import { describe, expect, it } from "vitest";

describe("the linked SDK", () => {
  it("loads through its package name", async () => {
    const sdk = await import("@joinedcontext/sdk");
    expect(sdk.parseGridConfig, "the SDK's own entry point").toBeTypeOf("function");
  });

  it("loads the components whose stylesheet lives outside ui/", async () => {
    // `EntityGrid` imports `./grid.css`; a stylesheet is served over the same `/@fs/` path as
    // the module, so it is the first thing a narrowed allow-list breaks.
    const grid = await import("@joinedcontext/sdk");
    expect(grid.EntityCompare, "a component whose module imports a sibling stylesheet").toBeTypeOf(
      "function",
    );
  });
});
