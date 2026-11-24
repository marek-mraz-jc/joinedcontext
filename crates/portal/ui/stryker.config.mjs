// Mutation testing of the Portal UI's pure logic (T-7592). The nightly `mutants-ui` job in
// .github/workflows/coverage.yml runs one group of stryker.groups.json per weekday and keeps
// the incremental file between nights, so a night re-tests only the mutants its changes touched.
// STRYKER_GROUP=<1..7> picks one group (the job sets it); unset mutates the whole scope.
// Scope: modules with no DOM and no network; .tsx stays out (the Playwright and render tests own it),
// as do src/api/client.ts (the fetch wrapper) and the generated src/api/schema.d.ts.
// Locally: `STRYKER_GROUP=6 pnpm exec stryker run`, or `pnpm exec stryker run --mutate src/units.ts`.

import { readFileSync } from "node:fs";

/**
 * Weekday (1 Monday .. 7 Sunday) to the modules that night mutates and the group's floor: the
 * mutation score under which the run fails, raised by the branch that kills mutants (a ratchet like
 * .github/coverage-ratchet.toml; null until the group is first measured). Shared with
 * vitest.stryker.config.ts. The goal is 100 with each equivalent mutant marked in its source by
 * `// Stryker disable next-line <mutator>: <reason>`.
 */
const GROUPS = JSON.parse(readFileSync(new URL("./stryker.groups.json", import.meta.url), "utf8"));
const group = GROUPS[process.env.STRYKER_GROUP ?? ""];

/** @type {import('@stryker-mutator/api/core').PartialStrykerOptions} */
export default {
  testRunner: "vitest",
  plugins: ["@stryker-mutator/vitest-runner"],
  // vitest.stryker.config.ts picks the tests/ files that import a mutated module by its path.
  vitest: { configFile: "vitest.stryker.config.ts", related: false },
  coverageAnalysis: "perTest",
  mutate: [
    ...(group?.mutate ?? Object.values(GROUPS).flatMap((g) => g.mutate)),
    "!**/*.tsx",
    "!src/api/client.ts",
    "!src/api/schema.d.ts",
  ],
  // The tests import ../../sdk (crates/portal/sdk), which a sandbox copy of ui/ cannot resolve, so
  // Stryker mutates the files in place and restores them when it ends.
  inPlace: true,
  // vitest strips types without checking them, so no file needs Stryker's `// @ts-nocheck` header;
  // without it an in-place run rewrites only the modules it mutates.
  disableTypeChecks: false,
  checkers: [],
  incremental: true,
  incrementalFile: "reports/stryker-incremental.json",
  ignoreStatic: true,
  concurrency: 3,
  timeoutMS: 20000,
  // The initial run executes every tests/ file that imports a mutated module, some of which render whole pages.
  dryRunTimeoutMinutes: 30,
  reporters: ["clear-text", "progress", "json", "html"],
  jsonReporter: { fileName: "reports/mutation/mutation.json" },
  htmlReporter: { fileName: "reports/mutation/mutation.html" },
  thresholds: { high: 100, low: 99, break: group?.break ?? null },
};
