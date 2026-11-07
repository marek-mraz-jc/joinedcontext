/**
 * Re-record what the T-2136 gates hold the tree to (UI-15, TS-19).
 *
 * `tests/controls.json` is an inventory of the controls each module declares and the tests that
 * name it. It is derived from the tree, so every merge that adds a control or covers a module
 * leaves it a merge behind and the required `ci` lane red until somebody edits the JSON by hand
 * — which is how `main` went red on run 35526638950 (T-2430). This writes the file the gate
 * would have to be given, the way `cargo test -- --ignored write_openapi_json` writes the
 * Portal's OpenAPI document.
 *
 *     pnpm record:gates              # writes tests/controls.json
 *     pnpm record:gates -- --check   # writes nothing, exits 1 if it is stale
 *
 * It records; it never decides. A module with controls and no test is still the module gate's
 * failure, and `--check` is what CI would run if this is ever wired into a lane.
 */
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { controlsIn, testsNaming, type Tree } from "../tests/gates.ts";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const inventory = join(ui, "tests/controls.json");

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
const naming = testsNaming(tree);

const modules = Object.fromEntries(
  Object.entries(tree.modules)
    .filter(([module]) => module.endsWith(".tsx"))
    .map(([module, source]) => [module, controlsIn(source)] as const)
    .filter(([, controls]) => Object.keys(controls).length > 0)
    // The file's own order: plain string order, not a locale's, so a rerun moves nothing.
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([module, controls]) => [
      module,
      { controls, tests: (naming.get(module) ?? []).map((file) => file.replace(/^tests\//, "")) },
    ]),
);

const current = JSON.parse(readFileSync(inventory, "utf8")) as Record<string, unknown>;
const written = `${JSON.stringify({ ...current, modules }, null, 2)}\n`;

if (process.argv.includes("--check")) {
  if (written !== readFileSync(inventory, "utf8")) {
    console.error("tests/controls.json is a merge behind the tree: run node scripts/record-gates.ts");
    process.exit(1);
  }
  console.log("tests/controls.json matches the tree");
} else {
  writeFileSync(inventory, written);
  console.log(`recorded ${Object.keys(modules).length} modules in tests/controls.json`);
}
