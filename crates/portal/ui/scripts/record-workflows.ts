/**
 * Re-record the workflow matrix the T-2728 gate holds the tree to (TS-26, AG-87).
 *
 * The matrix is written once, in `Testing/07-workflow-coverage.md` of the docs repository, and
 * `tests/workflow_coverage.json` is that page machine-read, so the gate needs no docs checkout.
 *
 *     pnpm record:workflows ../../docs/Testing/07-workflow-coverage.md           # writes the JSON
 *     pnpm record:workflows ../../docs/Testing/07-workflow-coverage.md --check   # exits 1 if stale
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseCoverage } from "../tests/gates.ts";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = join(ui, "tests/workflow_coverage.json");
const [page, flag] = process.argv.slice(2);
if (page === undefined) {
  console.error("usage: record-workflows <docs>/Testing/07-workflow-coverage.md [--check]");
  process.exit(2);
}

const recorded = `${JSON.stringify(parseCoverage(readFileSync(page, "utf8")), null, 2)}\n`;
if (flag === "--check") {
  const current = readFileSync(target, "utf8");
  if (current !== recorded) {
    console.error(`${target} is not what ${page} says: run pnpm record:workflows ${page}`);
    process.exit(1);
  }
} else {
  writeFileSync(target, recorded);
}
