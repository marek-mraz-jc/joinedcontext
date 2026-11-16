/**
 * The release suite clicks through a real stack: a route, a mock or a stub in it, or in a module it
 * imports, would let a journey pass on an answer jc never gave. This spec reads every file of
 * e2e/release and the relative modules they import, and fails the run (the journeys project depends
 * on it) when one routes, fulfils, aborts or replays a request.
 */
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

const SELF = fileURLToPath(import.meta.url);
const ROOT = dirname(SELF);
const REFUSED = [
  /\.route\s*\(/,
  /\.routeFromHAR\s*\(/,
  /\.routeWebSocket\s*\(/,
  /\.unroute(All)?\s*\(/,
  /\.fulfill\s*\(/,
  /\.abort\s*\(/,
  /\bserviceWorkers\s*:\s*["']block/,
  /\bmock[A-Z]\w*\s*\(/,
  /\bvi\.(mock|fn|spyOn)\b/,
];

/** `file` and every relative module it imports, transitively. */
function closure(file: string, seen = new Set<string>()): Set<string> {
  if (seen.has(file)) return seen;
  seen.add(file);
  for (const m of readFileSync(file, "utf8").matchAll(/from\s+["'](\.{1,2}\/[^"']+)["']/g)) {
    const base = resolve(dirname(file), m[1]);
    const found = [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts")].find((p) => existsSync(p) && p.match(/\.tsx?$/));
    if (found) closure(found, seen);
  }
  return seen;
}

export function offences(files: Iterable<string>): string[] {
  const out: string[] = [];
  for (const f of files) {
    if (f === SELF) continue;
    readFileSync(f, "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (REFUSED.some((re) => re.test(line))) out.push(`${relative(ROOT, f)}:${i + 1}: ${line.trim()}`);
      });
  }
  return out;
}

test("the release suite stubs nothing: no route, fulfil, abort or mock in it or its imports", () => {
  const files = new Set<string>();
  for (const f of readdirSync(ROOT)) if (/\.tsx?$/.test(f)) for (const g of closure(join(ROOT, f))) files.add(g);
  expect(files.size).toBeGreaterThan(2);
  expect(offences(files)).toEqual([]);
});

test("the lint catches a stub (red proof on a planted line)", () => {
  const planted = join(ROOT, "..", "..", "test-results", "release-lint-planted.ts");
  const lines = ['await page.route("**/api/**", (r) => r.fulfill({ json: [] }));', "await context.routeFromHAR('x.har');"];
  // Judged in memory: the planted text never lands in the suite's directory.
  const hits = lines.filter((l) => REFUSED.some((re) => re.test(l)));
  expect(hits).toHaveLength(2);
  expect(planted).toContain("release-lint-planted");
});

test("catalogue.json and the journeys name each other: every id once, each in the file it names", () => {
  type Entry = { id: string; file: string; title: string; nav?: string; requirements: string[]; pages: string[] };
  const { journeys } = JSON.parse(readFileSync(join(ROOT, "catalogue.json"), "utf8")) as { journeys: Entry[] };
  const ids = journeys.map((j) => j.id);
  expect(ids.filter((id, i) => ids.indexOf(id) !== i)).toEqual([]);
  const inFiles = new Map<string, string>();
  for (const f of readdirSync(ROOT).filter((x) => x.endsWith(".spec.ts"))) {
    for (const m of readFileSync(join(ROOT, f), "utf8").matchAll(/\btest\(\s*(["'`])(J\d{3}) (.+?)\1/g)) inFiles.set(m[2], `${f}|${m[3]}`);
  }
  for (const j of journeys) {
    expect(j.requirements.every((r) => /^[A-Z]+-\d+$/.test(r)), `${j.id} requirement ids`).toBe(true);
    // pages.spec.ts makes one test per catalogue entry with a `nav`.
    if (j.file === "pages.spec.ts") expect(j.nav, `${j.id} has no nav link`).toBeTruthy();
    else expect(inFiles.get(j.id), `${j.id} in ${j.file}`).toBe(`${j.file}|${j.title}`);
  }
  expect([...inFiles.keys()].filter((id) => !ids.includes(id))).toEqual([]);
});
