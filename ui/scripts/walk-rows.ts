#!/usr/bin/env node
/**
 * T-3986: fills the rows of tests/human_ui_walk.json from a live dialog walk.
 *
 *   node scripts/walk-rows.ts <walk dir> <walkedBy> [--check]
 *
 * <walk dir> holds what `e2e/live/walk-dialogs.spec.ts` writes with SHOTS_DIR set:
 * walk-dialogs-steward.json (the pages walked and the findings) and shots.jsonl (one line per
 * dialog screenshot). A module counts as walked when a page the walk opened renders it and that
 * page has no finding. "Renders it" is the static import graph from the route's components in
 * src/router.tsx, with two cuts so the graph claims no more than the walk saw: the walk never
 * enters `Shell` (the navigation and the assistant dock lie outside `main`, which the walk
 * presses), and a module under src/pages/<kind>/ counts only when an address of that kind was
 * walked, because the section route draws every kind. Rows already filled are kept. The
 * production build has no source maps, so the browser cannot name the module a control came
 * from; this graph is the closest the walk can prove.
 *
 * --check prints what would be filled and writes nothing.
 */
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SKIP = new Set(["src/components/layout/Shell.tsx"]);

/** Every .ts/.tsx module under src, as "src/…" paths. */
function sources(directory = join(ui, "src")): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [relative(ui, path)] : [];
  });
}

/** The module a relative specifier names from `from`, or undefined for a package or an asset. */
function resolveImport(from: string, specifier: string): string | undefined {
  if (!specifier.startsWith(".")) return undefined;
  const base = join(dirname(from), specifier);
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts"), join(base, "index.tsx")]) {
    if (/\.tsx?$/.test(candidate) && existsSync(join(ui, candidate))) return candidate;
  }
  return undefined;
}

/** The modules each module imports, static and dynamic, type-only imports left out. */
export type Graph = Map<string, Set<string>>;

export function importGraph(
  modules: string[],
  read = (module: string): string => readFileSync(join(ui, module), "utf8"),
): Graph {
  const graph: Graph = new Map();
  for (const module of modules) {
    const text = read(module);
    const targets = new Set<string>();
    for (const match of text.matchAll(/(?:^|\n)\s*(import|export)(\s+type)?[^;]*?\sfrom\s+"([^"]+)"|import\(\s*"([^"]+)"\s*\)/g)) {
      if (match[2]) continue;
      const target = resolveImport(module, match[3] ?? match[4]);
      if (target) targets.add(target);
    }
    graph.set(module, targets);
  }
  return graph;
}

/** Everything reachable from `roots`, never entering a module in SKIP. */
export function closure(graph: Graph, roots: string[], skip: Set<string> = SKIP): Set<string> {
  const seen = new Set<string>();
  const stack = roots.filter((root) => !skip.has(root));
  while (stack.length > 0) {
    const module = stack.pop() as string;
    if (seen.has(module)) continue;
    seen.add(module);
    for (const next of graph.get(module) ?? []) if (!skip.has(next)) stack.push(next);
  }
  return seen;
}

/**
 * The routes of router.tsx: full path and the modules its component draws. A block's identifiers
 * are followed into the router's own functions (DetailPage, …) and then to the module each one
 * is imported from, statically or by `lazy(() => import(…))`.
 */
export interface Route {
  name: string;
  path: string;
  modules: Set<string>;
}

export function routes(text: string, from = "src/router.tsx"): Route[] {
  const imported = new Map<string, string>();
  for (const match of text.matchAll(/import\s+(?!type)\{([^}]+)\}\s+from\s+"([^"]+)"/g)) {
    const module = resolveImport(from, match[2]);
    if (!module) continue;
    for (const name of match[1].split(",")) imported.set(name.trim().split(/\s+as\s+/).pop() ?? "", module);
  }
  for (const match of text.matchAll(/(?:^|\n)import\s+(\w+)\s+from\s+"([^"]+)"/g)) {
    const module = resolveImport(from, match[2]);
    if (module) imported.set(match[1], module);
  }
  for (const match of text.matchAll(/const\s+(\w+)\s*=\s*lazy\(\s*\(\)\s*=>\s*import\("([^"]+)"\)/g)) {
    const module = resolveImport(from, match[2]);
    if (module) imported.set(match[1], module);
  }
  const functions = new Map<string, string>();
  for (const match of text.matchAll(/\nfunction\s+(\w+)\s*\(([\s\S]*?)\n\}\n/g)) functions.set(match[1], match[2]);

  const modulesOf = (body: string, seen = new Set<string>()): Set<string> => {
    const found = new Set<string>();
    for (const [, name] of body.matchAll(/\b([A-Z]\w*)\b/g)) {
      const module = imported.get(name);
      const inner = functions.get(name);
      if (module) found.add(module);
      else if (inner !== undefined && !seen.has(name)) {
        seen.add(name);
        for (const each of modulesOf(inner, seen)) found.add(each);
      }
    }
    return found;
  };

  const blocks = new Map<string, { parent?: string; path: string; modules: Set<string> }>();
  for (const match of text.matchAll(/const\s+(\w+)\s*=\s*createRoute\(\{([\s\S]*?)\n\}\);/g)) {
    const body = match[2];
    blocks.set(match[1], {
      parent: body.match(/getParentRoute:\s*\(\)\s*=>\s*(\w+)/)?.[1],
      path: body.match(/\n\s*path:\s*"([^"]*)"/)?.[1] ?? "",
      modules: modulesOf(body),
    });
  }
  const fullPath = (name: string | undefined): string => {
    const block = name === undefined ? undefined : blocks.get(name);
    if (!block) return "";
    const own = block.path;
    if (own.startsWith("/") || !block.parent) return own === "/" && block.parent ? fullPath(block.parent) : own;
    return `${fullPath(block.parent).replace(/\/$/, "")}/${own}`;
  };
  // A parent draws its children (the section route draws `$plural/$name`), so a route renders
  // its own modules and those of every route above it.
  const drawn = (name: string | undefined): Set<string> => {
    const block = name === undefined ? undefined : blocks.get(name);
    if (!block) return new Set();
    return new Set([...block.modules, ...drawn(block.parent)]);
  };
  return [...blocks.entries()]
    .filter(([, block]) => block.path !== "")
    .map(([name]) => ({ name, path: fullPath(name), modules: drawn(name) }));
}

/** The most specific route an address opens: the one with the most literal segments. */
export function routeOf(address: string, table: Route[]): Route | undefined {
  const path = address.split(/[?#]/)[0].replace(/\/$/, "") || "/";
  const segments = path.split("/");
  let best: { route: Route; literal: number } | undefined;
  for (const route of table) {
    const pattern = route.path.replace(/\/$/, "").split("/");
    if (pattern.length !== segments.length) continue;
    if (!pattern.every((part, index) => part.startsWith("$") || part === segments[index])) continue;
    const literal = pattern.filter((part) => !part.startsWith("$")).length;
    if (!best || literal > best.literal) best = { route, literal };
  }
  return best?.route;
}

/** The kind folder a section address names: /projects/p/<plural>/… → src/pages/<plural>/. */
const kindOf = (address: string): string | undefined => address.match(/^\/projects\/[^/]+\/([^/?#]+)/)?.[1];

export interface Walked {
  walked: string[];
  report: { address: string; findings: string[] }[];
  shots: { file: string; route: string; state?: string }[];
}

/** The modules the walk proves, each with the screenshots of the pages that drew it. */
export function rows({ walked, report, shots }: Walked, table: Route[], graph: Graph, folders: Set<string>): Map<string, string[]> {
  const failed = new Set(report.filter((entry) => entry.findings.length > 0).map((entry) => entry.address));
  const kinds = new Set(walked.map(kindOf).filter((kind): kind is string => kind !== undefined));
  const rendered = new Map<string, Set<string>>();
  for (const address of walked) {
    if (failed.has(address)) continue;
    const route = routeOf(address, table);
    if (!route) continue;
    for (const module of closure(graph, [...route.modules])) {
      const folder = module.match(/^src\/pages\/([^/]+)\//)?.[1];
      if (folder && folders.has(folder) && !kinds.has(folder) && route.path.includes("$plural")) continue;
      const addresses = rendered.get(module) ?? new Set<string>();
      addresses.add(address);
      rendered.set(module, addresses);
    }
  }
  const result = new Map<string, string[]>();
  for (const [module, addresses] of rendered) {
    result.set(
      module,
      shots.filter((shot) => addresses.has(shot.route)).map((shot) => shot.file),
    );
  }
  return result;
}

/**
 * The frame's modules, from the shell pass (walk-shell-steward.json): everything Shell draws, when
 * no control of it had a finding, with the pass's screenshots. Nothing from a pass with a finding.
 */
export function shellRows({ report, shots }: Pick<Walked, "report" | "shots">, graph: Graph): Map<string, string[]> {
  if (report.length === 0 || report.some((entry) => entry.findings.length > 0)) return new Map();
  const files = shots.filter((shot) => shot.state?.startsWith("shell-")).map((shot) => shot.file);
  const shell = [...SKIP][0];
  return new Map([...closure(graph, [shell], new Set())].map((module) => [module, files]));
}

function main(argv: string[]): number {
  const [directory, walkedBy, flag] = argv;
  if (!directory || !walkedBy) {
    console.error("usage: node scripts/walk-rows.ts <walk dir> <walkedBy> [--check]");
    return 2;
  }
  const read = (name: string) =>
    existsSync(join(directory, name)) ? (JSON.parse(readFileSync(join(directory, name), "utf8")) as Pick<Walked, "walked" | "report">) : undefined;
  const pages = read("walk-dialogs-steward.json");
  const frame = read("walk-shell-steward.json");
  if (!pages && !frame) {
    console.error(`${directory} holds neither walk-dialogs-steward.json nor walk-shell-steward.json`);
    return 2;
  }
  const shotsFile = join(directory, "shots.jsonl");
  const shots = existsSync(shotsFile)
    ? readFileSync(shotsFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line) as Walked["shots"][number])
    : [];
  const table = routes(readFileSync(join(ui, "src/router.tsx"), "utf8"));
  const graph = importGraph(sources());
  const folders = new Set(readdirSync(join(ui, "src/pages")));
  // The page walker's findings (walker-<who>.json) fail a page too: a clean dialog walk is not enough.
  const judged = ["steward", "viewer"].flatMap((who) => {
    const file = join(directory, `walker-${who}.json`);
    return existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as Walked["report"]) : [];
  });
  const found = pages ? rows({ ...pages, report: [...pages.report, ...judged], shots }, table, graph, folders) : new Map<string, string[]>();
  if (frame) for (const [module, files] of shellRows({ report: frame.report, shots }, graph)) if (!found.has(module)) found.set(module, files);

  const file = join(ui, "tests/human_ui_walk.json");
  const walk = JSON.parse(readFileSync(file, "utf8")) as {
    modules: Record<string, { walkedBy: string; date: string; screenshots: string[] } | null>;
  };
  const date = new Date().toISOString().slice(0, 10);
  // Rows name the shots as the task files them: evidence/<agent>/T-3986/<run>/<file>.
  const absolute = resolve(directory);
  const evidence = absolute.includes("/evidence/") ? absolute.slice(absolute.indexOf("/evidence/") + 1) : absolute;
  let filled = 0;
  for (const module of Object.keys(walk.modules)) {
    if (walk.modules[module] !== null || !found.has(module)) continue;
    walk.modules[module] = {
      walkedBy,
      date,
      // A shared component is drawn on most pages: five shots show it, more only bury the file.
      screenshots: (found.get(module) ?? []).slice(0, 5).map((name) => `${evidence}/${name}`),
    };
    filled += 1;
  }
  const open = Object.keys(walk.modules).filter((module) => walk.modules[module] === null);
  console.log(`${filled} row(s) filled, ${open.length} module(s) still unwalked`);
  for (const module of open) console.log(`  unwalked: ${module}`);
  if (flag !== "--check") writeFileSync(file, `${JSON.stringify(walk, null, 2)}\n`);
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exit(main(process.argv.slice(2)));
