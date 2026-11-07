/**
 * T-2136: every control a person can operate is recorded and its module has a test (UI-15, UI-16, TS-19).
 *
 * `controls.json` is the inventory: per module, the controls it declares and the tests that name
 * it. This gate reads the source and compares. A module that gains a button, a switch or a text
 * field has changed what a person can do, so the build goes red until the inventory records it
 * and a test names the module.
 *
 * Why it reads the source rather than rendering every route: a render needs each page's own
 * answers to reach the state its controls live in. Measured on 2026-09-20, rendering the pages
 * with the harness's default answers puts several of them in their error or empty frame (the
 * approvals page renders one control, "Hide Error"), so the inventory would record the error
 * frames rather than the Portal. Each page's own test renders it with data, and this gate holds
 * the inventory to the source and names those tests. The accessible name of each control is
 * asserted where it is rendered: the page contract's axe run and the per-page tests.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { controlsIn, testsNaming, type Tree } from "./gates";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Modules with controls and no test yet. They are the module gate's list, not a second one. */
const UNTESTED_ON_2026_09_20 = 1;

function walk(directory: string): string[] {
  return readdirSync(join(ui, directory), { withFileTypes: true }).flatMap((entry) => {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) return walk(path);
    return /\.tsx?$/.test(entry.name) ? [path] : [];
  });
}

function read(files: string[]): Record<string, string> {
  return Object.fromEntries(files.map((file) => [file, readFileSync(join(ui, file), "utf8")]));
}

const tree: Tree = { modules: read(walk("src")), tests: read(walk("tests")) };
const recorded = (
  JSON.parse(readFileSync(join(ui, "tests/controls.json"), "utf8")) as {
    modules: Record<string, { controls: Record<string, number>; tests: string[] }>;
  }
).modules;
const allowed = Object.keys(
  (
    JSON.parse(readFileSync(join(ui, "tests/gate_modules.allow.json"), "utf8")) as {
      modules: Record<string, string>;
    }
  ).modules,
);

const found = Object.fromEntries(
  Object.entries(tree.modules)
    .filter(([module]) => module.endsWith(".tsx"))
    .map(([module, source]) => [module, controlsIn(source)])
    .filter(([, controls]) => Object.keys(controls).length > 0),
) as Record<string, Record<string, number>>;

describe("the control gate (T-2136)", () => {
  it("records every module that has controls, and no module that lost them", () => {
    expect(
      Object.keys(found).filter((module) => !(module in recorded)).sort(),
      "these modules declare controls that tests/controls.json does not record",
    ).toEqual([]);
    expect(
      Object.keys(recorded).filter((module) => !(module in found)).sort(),
      "these are recorded but declare no control any more: remove them from tests/controls.json",
    ).toEqual([]);
  });

  it("records what each module's controls are", () => {
    const changed = Object.entries(found)
      .filter(([module, controls]) => {
        const entry = recorded[module];
        return entry && JSON.stringify(entry.controls) !== JSON.stringify(controls);
      })
      .map(([module, controls]) => `${module}: ${JSON.stringify(controls)}`);
    expect(
      changed,
      "the controls of these modules changed; record them in tests/controls.json with the test that uses each",
    ).toEqual([]);
  });

  it("names a test for every module with controls, and lists the ones that have none", () => {
    const naming = testsNaming(tree);
    const untested = Object.keys(found).filter(
      (module) => (naming.get(module) ?? []).length === 0,
    );
    expect(
      untested.filter((module) => !allowed.includes(module)).sort(),
      "these have controls and no test; write one, or list the module in tests/gate_modules.allow.json",
    ).toEqual([]);
    expect(untested.length).toBeLessThanOrEqual(UNTESTED_ON_2026_09_20);
    for (const [module, entry] of Object.entries(recorded)) {
      const naming_ = (naming.get(module) ?? []).map((file) => file.replace(/^tests\//, ""));
      expect(entry.tests, `tests/controls.json is stale for ${module}`).toEqual(naming_);
    }
  });
});

describe("the rule the control gate applies", () => {
  it("counts the shared controls and the plain form elements, by kind", () => {
    expect(
      controlsIn('<Button>a</Button><button type="submit" /><Switch /><input /><select />'),
    ).toEqual({ Button: 1, Switch: 1, button: 1, input: 1, select: 1 });
  });

  it("counts a control that is added and says so by the count", () => {
    const before = controlsIn("<Button>a</Button>");
    const after = controlsIn("<Button>a</Button><Button>b</Button>");
    expect(before).toEqual({ Button: 1 });
    expect(after).toEqual({ Button: 2 });
    expect(JSON.stringify(before) === JSON.stringify(after)).toBe(false);
  });

  it("reads no control out of a comment or out of prose", () => {
    expect(controlsIn("/** <Button>a</Button> */")).toEqual({});
    expect(controlsIn("// <input />")).toEqual({});
    expect(controlsIn("<ButtonGroup />")).toEqual({});
  });

  it("names the tests of a module and of what a barrel re-exports", () => {
    const naming = testsNaming({
      modules: {
        "src/ui/index.ts": 'export { Button } from "./Button";',
        "src/ui/Button.tsx": "export const Button = () => null;",
        "src/page.tsx": 'import { Button } from "./ui";\nexport const Page = () => null;',
      },
      tests: {
        "tests/button.test.tsx": 'import { Button } from "../src/ui";',
        "tests/helper.ts": 'import { Button } from "../src/ui";',
      },
    });
    expect(naming.get("src/ui/Button.tsx"), "the barrel carries the test to the component").toEqual([
      "tests/button.test.tsx",
    ]);
    expect(naming.get("src/page.tsx"), "a module no test imports is named by none").toBeUndefined();
  });
});
