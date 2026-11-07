// The weight every page of the Portal pays before it answers (T-3280): the entry chunk the
// built index.html loads, gzipped. A route's own code loads with the route; what is left in the
// entry is what every visit downloads first. A public page (the catalogue, a dataset, the
// accessibility statement) has a budget of its own for what its route adds (T-5122): a citizen on
// a phone opens it cold. Over a budget, the build lane goes red with the chunk named, so a static
// import of a heavy page is caught the day it lands.
//
//   node scripts/bundle-budget.mjs ui/dist
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

/**
 * KB, gzipped. Measured 1 679 on 2026-10-08 before T-3280 and 978 after the heavy pages, the form
 * engine and the map loaded with their routes; 1 000 before and 729 after T-3316 loaded only the
 * chosen language, 493 once the create and edit dialogs loaded with their first opening, 385 once
 * the signed-in shell loaded with the first signed-in page, 146 once a public page booted its own
 * router with its own words and the shared controls became tree-shakable (T-5122). The full
 * Portal's own code loads after the entry (main.tsx). The budget is the last measure plus 5 %.
 */
export const ENTRY_BUDGET_KB = 154;

/**
 * KB gzipped that each public route loads beyond the entry, keyed by its chunk's name: measured on
 * 2026-10-22 (T-5122; PublicView and FormView 2026-10-24, T-5165) plus 5 %, and at least 2 KB so a one-line change does not flap the lane.
 */
export const ROUTE_BUDGETS_KB = {
  CataloguePage: 26,
  DatasetPage: 10,
  AccessibilityPage: 3,
  GlossaryPage: 4,
  PublicView: 15,
  FormView: 28,
};

/**
 * The web workers the Portal needs: Monaco's editor worker (YAML needs no language service of its
 * own) and MapLibre's. A TypeScript, CSS, HTML or JSON worker means Monaco is imported whole again
 * (T-5513: 8.8 MB of the build).
 */
export const WORKERS = ["editor.worker", "maplibre-gl-worker"];

/** The worker chunks in `names` that WORKERS does not list. */
export function strayWorkers(names) {
  return names.filter((name) => /\.worker-[^/]*\.js$|-worker-[^/]*\.js$/.test(name) && !WORKERS.some((w) => name.startsWith(`${w}-`)));
}

/** The scripts index.html loads at once: the entry, and the chunks it preloads. */
export function entryScripts(html) {
  return [...html.matchAll(/<(?:script[^>]*\ssrc|link[^>]*rel="modulepreload"[^>]*\shref)="\/?(assets\/[^"]+\.js)"/g)].map((m) => m[1]);
}

/** The chunks a chunk imports statically (`from"./x.js"`, `import"./x.js"`), not its lazy ones. */
export function staticImports(code) {
  return [...code.matchAll(/(?:from|import)\s*"\.\/([^"]+\.js)"/g)].map((m) => `assets/${m[1]}`);
}

/** `start` and every chunk it reaches through static imports, read by `read(path)`. */
export function closure(start, read) {
  const seen = new Set();
  const todo = [start];
  while (todo.length > 0) {
    const chunk = todo.pop();
    if (seen.has(chunk)) continue;
    seen.add(chunk);
    todo.push(...staticImports(read(chunk)));
  }
  return seen;
}

export function weightKb(bytes) {
  return Math.round(gzipSync(bytes).length / 1024);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const dist = process.argv[2];
  if (!dist) {
    console.error("usage: node scripts/bundle-budget.mjs <ui/dist>");
    process.exit(2);
  }
  const scripts = entryScripts(readFileSync(join(dist, "index.html"), "utf8"));
  if (scripts.length === 0) {
    console.error("index.html loads no script: the build is not what this budget measures");
    process.exit(1);
  }
  const weights = scripts.map((script) => [script, weightKb(readFileSync(join(dist, script)))]);
  const total = weights.reduce((sum, [, kb]) => sum + kb, 0);
  for (const [script, kb] of weights) console.log(`${String(kb).padStart(6)} KB gz  ${script}`);
  console.log(`${String(total).padStart(6)} KB gz  every page, before it answers (budget ${ENTRY_BUDGET_KB})`);
  let over = total > ENTRY_BUDGET_KB;
  if (over) {
    console.error(`over the budget by ${total - ENTRY_BUDGET_KB} KB: load the heavy page lazily (router.tsx) instead of raising the number`);
  }
  const read = (chunk) => readFileSync(join(dist, chunk), "utf8");
  const entry = new Set(scripts.flatMap((script) => [...closure(script, read)]));
  const chunks = readdirSync(join(dist, "assets"));
  const stray = strayWorkers(chunks);
  if (stray.length > 0) {
    console.error(`workers no editor uses: ${stray.join(", ")}: import only the Monaco parts monaco-setup.ts needs`);
    over = true;
  }
  for (const [route, budget] of Object.entries(ROUTE_BUDGETS_KB)) {
    const chunk = chunks.find((name) => name.startsWith(`${route}-`) && name.endsWith(".js"));
    if (!chunk) {
      console.error(`no ${route}-*.js chunk: a public page is no longer lazy, or ROUTE_BUDGETS_KB names a route that is gone`);
      over = true;
      continue;
    }
    const own = [...closure(`assets/${chunk}`, read)].filter((c) => !entry.has(c));
    const kb = own.reduce((sum, c) => sum + weightKb(readFileSync(join(dist, c))), 0);
    console.log(`${String(kb).padStart(6)} KB gz  the ${route} route beyond the entry (budget ${budget})`);
    if (kb > budget) {
      console.error(`${route} is over its budget by ${kb - budget} KB: ${own.join(", ")}`);
      over = true;
    }
  }
  if (over) process.exit(1);
}
