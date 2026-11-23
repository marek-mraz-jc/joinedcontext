import { globSync, readFileSync } from "node:fs";
import base from "./vite.config";

// Stryker's test run (T-7592): only the tests/ files that import a mutated module by its own path.
// vitest's `related` mode follows the whole import graph instead, and a helper such as
// navigation/shortcuts is reachable from almost every page test, so the initial run did not end in
// 30 minutes; the page tests stay with the fast lane and the coverage job.
const groups: Record<string, { mutate: string[] }> = JSON.parse(readFileSync("stryker.groups.json", "utf8"));
const patterns = groups[process.env.STRYKER_GROUP ?? ""]?.mutate ?? Object.values(groups).flatMap((g) => g.mutate);
const skipped = new Set(patterns.filter((p) => p.startsWith("!")).flatMap((p) => globSync(p.slice(1))));
const modules = patterns
  .filter((p) => !p.startsWith("!"))
  .flatMap((p) => globSync(p))
  .filter((f) => !skipped.has(f))
  .map((f) => f.replace(/\.ts$/, ""));
const imports = (test: string) => {
  const source = readFileSync(test, "utf8");
  return modules.some((m) => source.includes(`../${m}"`) || source.includes(`../${m}.ts"`));
};

// Replaced, not merged: mergeConfig would append to the base config's include list.
export default { ...base, test: { ...base.test, include: globSync("tests/**/*.test.{ts,tsx}").filter(imports) } };
