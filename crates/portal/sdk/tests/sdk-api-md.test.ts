import { describe, expect, it } from "vitest";
import apiMd from "../API.md?raw";
import * as sdk from "../src/sdk/index";
import * as grid from "../src/sdk/grid";
import * as server from "../src/sdk/server";
import * as testing from "../src/sdk/testing";

// SDK-09: API.md is what the model reads, so an export it does not list is one the model never uses.
describe("API.md", () => {
  it.each([
    ["@joinedcontext/sdk", sdk],
    ["@joinedcontext/sdk/grid", grid],
    ["@joinedcontext/sdk/server", server],
    ["@joinedcontext/sdk/testing", testing],
  ])("lists every runtime export of %s", (_entry, exports) => {
    const missing = Object.keys(exports).filter((name) => !new RegExp(`\\b${name}\\b`).test(apiMd));
    expect(missing).toEqual([]);
  });

  // SDK-02: a model writes an app from API.md and believes it, so a name API.md declares that
  // its entry point does not export is an import that fails in the generated app.
  it.each([
    ["@joinedcontext/sdk", sdk],
    ["@joinedcontext/sdk/server", server],
    ["@joinedcontext/sdk/testing", testing],
  ])("declares nothing %s does not export", (entry, exports) => {
    const invented = declaredIn(entry).filter((name) => !(name in exports));
    expect(invented).toEqual([]);
  });

  // G-R2GRID2-001: the grid entry is the root's grid, so an App may import either and gets one component.
  it("lists @joinedcontext/sdk/grid as an import, with names the root entry also exports", () => {
    expect(apiMd).toMatch(/Allowed imports:[^\n]*`@joinedcontext\/sdk\/grid`/);
    expect(Object.keys(grid).filter((name) => !(name in sdk))).toEqual([]);
  });

  it("finds the declarations it checks", () => {
    expect(declaredIn("@joinedcontext/sdk")).toEqual(expect.arrayContaining(["jc", "ProblemError", "EntityGrid", "DEFAULT_PAGE_SIZE", "MAX_PAGE_SIZE"]));
  });

  it("stays within 12 000 tokens (4 characters per token, rounded up)", () => {
    expect(Math.ceil(apiMd.length / 4)).toBeLessThanOrEqual(12_000);
  });
});

/** The runtime names (`function`, `class`, `const`) declared in the ts blocks of one entry point's section. */
function declaredIn(entry: string): string[] {
  const start = apiMd.indexOf(`## Entry Point: \`${entry}\``);
  if (start < 0) return [];
  const next = apiMd.indexOf("\n## ", start + 1);
  const section = apiMd.slice(start, next < 0 ? undefined : next);
  const names = new Set<string>();
  for (const block of section.matchAll(/```ts\n([\s\S]*?)```/g)) {
    for (const line of block[1].split("\n")) {
      const declared = /^(?:async\s+)?(function|class|const)\s+([A-Za-z_$][\w$]*)/.exec(line.trim());
      if (!declared) continue;
      names.add(declared[2]);
      // `const A = 1, B = 2` declares both.
      if (declared[1] === "const") {
        for (const more of line.matchAll(/,\s*([A-Za-z_$][\w$]*)\s*[:=]/g)) names.add(more[1]);
      }
    }
  }
  return [...names];
}
