/**
 * T-2136: every module under `src/` is imported by at least one test (UI-15, TS-19).
 *
 * The owner's rule is that in the UI everything needs a test. Nothing enforced it, so a new
 * component could ship with no test and the lane stayed green. This gate reads the tree and
 * fails on a module no test names, unless `gate_modules.allow.json` lists it with the task that
 * closes it. That list only shrinks: an entry that is covered now, or names a module that is
 * gone, is a failure of its own, and the number of entries is held against what was measured on
 * 2026-09-20.
 *
 * Two modules are excepted by name, not by the list: `src/main.tsx` is the bundle entry and
 * `src/api/schema.d.ts` is generated from the Portal's OpenAPI document.
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { isBarrel, modulesNamedByTests, resolveSpecifier, verdict, type Tree } from "./gates";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Modules that are not a unit anybody can test: the entry point and the generated types. */
const EXCEPTED = ["src/main.tsx", "src/api/schema.d.ts"];

/** What was missing when this gate was written. The list may shrink; it may never grow. */
const ALLOWED_ON_2026_09_20 = 1;

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
const subjects = Object.keys(tree.modules).filter((module) => !EXCEPTED.includes(module));
const allowFile = JSON.parse(readFileSync(join(ui, "tests/gate_modules.allow.json"), "utf8")) as {
  modules: Record<string, string>;
};

describe("the module gate (T-2136)", () => {
  it("names every module of src/ in a test, or in the allow-list with the task that closes it", () => {
    const named = modulesNamedByTests(tree);
    const { missing, stale } = verdict(subjects, named, Object.keys(allowFile.modules));
    expect(
      missing,
      "no test under ui/tests imports these; write one, or add it to tests/gate_modules.allow.json with its task",
    ).toEqual([]);
    expect(
      stale,
      "these are covered now, or gone: remove them from tests/gate_modules.allow.json",
    ).toEqual([]);
  });

  it("keeps the allow-list shrinking and every entry answerable by a task", () => {
    const entries = Object.entries(allowFile.modules);
    expect(entries.length).toBeLessThanOrEqual(ALLOWED_ON_2026_09_20);
    for (const [module, task] of entries) {
      expect(task, `${module} names no task that will close it`).toMatch(/^T-\d{4}$/);
    }
  });

  it("excepts nothing that is not there", () => {
    for (const module of EXCEPTED) {
      expect(Object.keys(tree.modules), `${module} is excepted but absent`).toContain(module);
    }
  });
});

describe("the rule the module gate applies", () => {
  const modules = {
    "src/a.ts": "export const a = 1;",
    "src/ui/index.ts": 'export { B } from "./B";\nexport { C } from "./C";',
    "src/ui/B.tsx": "export const B = () => null;",
    "src/ui/C.tsx": "export const C = () => null;",
    "src/deep.ts": 'import { a } from "./a";\nexport const deep = a;',
  };

  it("counts a module a test imports directly", () => {
    const named = modulesNamedByTests({ modules, tests: { "tests/a.test.ts": 'import "../src/a";' } });
    expect([...named]).toContain("src/a.ts");
  });

  it("counts what a barrel re-exports, because every page imports its controls through one", () => {
    const named = modulesNamedByTests({ modules, tests: { "tests/b.test.tsx": 'import { B } from "../src/ui";' } });
    expect([...named].sort()).toEqual(["src/ui/B.tsx", "src/ui/C.tsx", "src/ui/index.ts"]);
  });

  it("does not count a module reached only through another module", () => {
    const named = modulesNamedByTests({
      modules,
      tests: { "tests/deep.test.ts": 'import "../src/deep";' },
    });
    expect(named.has("src/deep.ts")).toBe(true);
    expect(named.has("src/a.ts"), "a module a page imports is not a module a test names").toBe(false);
  });

  it("reads no import out of a comment", () => {
    const named = modulesNamedByTests({
      modules,
      tests: { "tests/c.test.ts": '/** see import { a } from "../src/a"; */\nexport {};' },
    });
    expect(named.size).toBe(0);
  });

  it("is red for a module nothing imports and green once a test does", () => {
    const subjectsHere = Object.keys(modules);
    const nothing = modulesNamedByTests({ modules, tests: {} });
    expect(verdict(subjectsHere, nothing, []).missing).toEqual(subjectsHere.sort());
    const allowed = subjectsHere;
    expect(verdict(subjectsHere, nothing, allowed).missing).toEqual([]);
    const covered = modulesNamedByTests({ modules, tests: { "tests/a.test.ts": 'import "../src/a";' } });
    expect(
      verdict(subjectsHere, covered, allowed).stale,
      "an allow-list entry that is covered now has to go",
    ).toContain("src/a.ts");
  });

  it("knows a barrel from a module that does work", () => {
    expect(isBarrel('export { B } from "./B";')).toBe(true);
    expect(isBarrel("/** doc */\nexport { B } from \"./B\";\nexport type { P } from \"./B\";")).toBe(true);
    expect(isBarrel("export const a = 1;\nconst hidden = 2;")).toBe(false);
    expect(isBarrel("")).toBe(false);
  });

  it("resolves a specifier the way the bundler does, or not at all", () => {
    const names = new Set(Object.keys(modules));
    expect(resolveSpecifier("tests/x.test.ts", "../src/a", names)).toBe("src/a.ts");
    expect(resolveSpecifier("tests/x.test.ts", "../src/ui", names)).toBe("src/ui/index.ts");
    expect(resolveSpecifier("tests/x.test.ts", "react", names)).toBeUndefined();
    expect(resolveSpecifier("tests/x.test.ts", "../src/gone", names)).toBeUndefined();
  });
});
