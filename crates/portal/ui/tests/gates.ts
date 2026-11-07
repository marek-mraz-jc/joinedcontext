/**
 * T-2136: the three inventories of the UI test gate (UI-15, UI-16, UI-44, TS-19).
 *
 * Every function here reads text and returns a verdict. Nothing renders, nothing touches the
 * network, and each one takes its input as an argument rather than reading the tree itself, so
 * the gate tests can hold the rule to a synthetic tree where the answer is known: a gate whose
 * own logic is untested is a gate that can stop biting without anybody noticing.
 *
 * The three gates that use these are `gate_modules.test.ts` (every module is imported by a
 * test), `gate_routes.test.ts` (every route is opened by a spec) and `gate_controls.test.tsx`
 * (every control is used by a test). Each carries an allow-list of what is still missing, and
 * each asserts that its list only ever shrinks.
 */

/** Every `from "…"` and `import("…")` specifier of one source file, in order. */
const SPECIFIER = /(?:from|import)\s*\(?\s*['"]([^'"]+)['"]/g;

export function importSpecifiers(source: string): string[] {
  return [...source.matchAll(SPECIFIER)].map((match) => match[1]);
}

/** Comments removed, so a specifier inside a doc comment is not an import. */
export function withoutComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

/**
 * A file that only re-exports: `components/ui/index.ts` is the one the Portal has, and every
 * page imports its controls through it (UI-01). A test that imports the barrel is a test that
 * names what the barrel re-exports, which is why the module gate follows one.
 */
export function isBarrel(source: string): boolean {
  const statements = withoutComments(source)
    .split(";")
    .map((statement) => statement.trim())
    .filter(Boolean);
  return statements.length > 0 && statements.every((statement) => statement.startsWith("export "));
}

/** The repository-relative module a relative specifier names, or `undefined` when it is a package. */
export function resolveSpecifier(
  fromFile: string,
  specifier: string,
  modules: ReadonlySet<string>,
): string | undefined {
  if (!specifier.startsWith(".")) return undefined;
  const parts = fromFile.split("/").slice(0, -1).concat(specifier.split("/"));
  const stack: string[] = [];
  for (const part of parts) {
    if (part === "." || part === "") continue;
    if (part === "..") stack.pop();
    else stack.push(part);
  }
  const base = stack.join("/");
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]) {
    if (modules.has(candidate)) return candidate;
  }
  return undefined;
}

export interface Tree {
  /** Every source module, repository-relative (`src/components/ui/Button.tsx`), with its text. */
  modules: Readonly<Record<string, string>>;
  /** Every test file, the same way (`tests/button_rules.test.tsx`). */
  tests: Readonly<Record<string, string>>;
}

/**
 * The modules at least one test imports, directly or through a barrel.
 *
 * Transitive imports do not count on purpose: a module reached only because a page imports it
 * is a module no test names, and naming it is what the per-file tasks of the `ui-*` groups do.
 */
export function modulesNamedByTests(tree: Tree): Set<string> {
  const names = new Set(Object.keys(tree.modules));
  const named = new Set<string>();
  const follow = (module: string): void => {
    if (named.has(module)) return;
    named.add(module);
    const source = tree.modules[module];
    if (!isBarrel(source)) return;
    for (const specifier of importSpecifiers(source)) {
      const target = resolveSpecifier(module, specifier, names);
      if (target) follow(target);
    }
  };
  for (const [file, source] of Object.entries(tree.tests)) {
    for (const specifier of importSpecifiers(withoutComments(source))) {
      const target = resolveSpecifier(file, specifier, names);
      if (target) follow(target);
    }
  }
  return named;
}

export interface Verdict {
  /** Uncovered and not in the allow-list: the build is red and these are why. */
  missing: string[];
  /** In the allow-list and covered now, or gone from the tree: the entry has to go. */
  stale: string[];
}

/** What the allow-list still has to carry, and what it may no longer carry. */
export function verdict(
  subjects: readonly string[],
  covered: ReadonlySet<string>,
  allowed: readonly string[],
): Verdict {
  const allowedSet = new Set(allowed);
  return {
    missing: subjects.filter((subject) => !covered.has(subject) && !allowedSet.has(subject)).sort(),
    stale: allowed.filter((entry) => !subjects.includes(entry) || covered.has(entry)).sort(),
  };
}

/**
 * Every `path:` of the router as the address it serves, in the order the file declares them.
 *
 * A child route's path is relative to its parent (`new` under `/projects/$project/$plural` is
 * `/projects/$project/$plural/new`, T-2474), and a child's index `/` is its parent's own page, so
 * it is not a route of its own.
 */
export function routerPaths(source: string): string[] {
  const full = new Map<string, string>();
  const paths: string[] = [];
  const route =
    /(?:const\s+(\w+)\s*=\s*)?createRoute\(\{\s*(?:getParentRoute:\s*\(\)\s*=>\s*(\w+),\s*)?(path|id):\s*"([^"]+)"/g;
  for (const [, name, parent, key, value] of withoutComments(source).matchAll(route)) {
    const base = parent === undefined ? undefined : full.get(parent);
    if (key === "id") {
      if (name) full.set(name, base ?? "");
      continue;
    }
    const nested = base !== undefined && base !== "" && !value.startsWith("/");
    const path = nested ? `${base}/${value}` : base !== undefined && base !== "" && value === "/" ? null : value;
    if (name) full.set(name, path ?? base ?? value);
    if (path !== null) paths.push(path);
  }
  return paths;
}

/** A router path as the matcher of an address: `$name` stands for one segment. */
export function routeMatcher(path: string): RegExp {
  const segments = path
    .split("/")
    .map((segment) =>
      segment.startsWith("$") ? "[^/]+" : segment.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
    );
  return new RegExp(`^${segments.join("/")}/?$`);
}

/**
 * Which route an address belongs to: the most specific one that matches it.
 *
 * `/projects/helsinki/models` matches both `/projects/$project/models` and the generic
 * `/projects/$project/$plural`, and the router resolves it to the first. Counting it for both
 * would let the generic route stand in for every page the Portal has.
 */
export function routeOf(address: string, paths: readonly string[]): string | undefined {
  const matches = paths.filter((path) => routeMatcher(path).test(address));
  if (matches.length === 0) return undefined;
  const parameters = (path: string) => path.split("/").filter((segment) => segment.startsWith("$")).length;
  return matches.sort(
    (a, b) => parameters(a) - parameters(b) || b.length - a.length,
  )[0];
}

/** Every address a spec names, from any string literal that starts with a slash. */
export function addressesIn(source: string): string[] {
  return [...withoutComments(source).matchAll(/["'`](\/[A-Za-z0-9$_./?&=%{}-]*)["'`]/g)].map((match) =>
    match[1].split("?")[0],
  );
}

/** Route -> the spec files that open it, for one set of specs. */
export function routesOpenedBy(
  paths: readonly string[],
  specs: Readonly<Record<string, string>>,
): Map<string, string[]> {
  const opened = new Map<string, string[]>();
  for (const [file, source] of Object.entries(specs)) {
    for (const address of addressesIn(source)) {
      const route = routeOf(address, paths);
      if (!route) continue;
      const list = opened.get(route) ?? [];
      if (!list.includes(file)) list.push(file);
      opened.set(route, list.sort());
    }
  }
  return opened;
}

/**
 * The controls one module declares, counted by kind.
 *
 * Every control a person operates in the Portal is one of these elements: the shared components
 * on Radix (`Button`, `Switch`, `Checkbox`, `RadioGroup`, `FilePicker`, a `Menu.Item`, a tab
 * trigger) or a plain form element where a page needs one. A module that gains one has changed
 * what a person can do, which is what `controls.json` records and the control gate compares.
 */
const CONTROL = /<(Button|button|Switch|Checkbox|RadioGroup|FilePicker|Menu\.Item|TabsTrigger|select|textarea|input)\b/g;

export function controlsIn(source: string): Record<string, number> {
  const counted: Record<string, number> = {};
  for (const match of withoutComments(source).matchAll(CONTROL)) {
    counted[match[1]] = (counted[match[1]] ?? 0) + 1;
  }
  return Object.fromEntries(Object.entries(counted).sort(([a], [b]) => a.localeCompare(b)));
}

/** Which test files name a module, directly or through a barrel. */
export function testsNaming(tree: Tree): Map<string, string[]> {
  const names = new Set(Object.keys(tree.modules));
  const naming = new Map<string, Set<string>>();
  const follow = (module: string, test: string): void => {
    const already = naming.get(module) ?? new Set<string>();
    if (already.has(test)) return;
    already.add(test);
    naming.set(module, already);
    if (!isBarrel(tree.modules[module])) return;
    for (const specifier of importSpecifiers(tree.modules[module])) {
      const target = resolveSpecifier(module, specifier, names);
      if (target) follow(target, test);
    }
  };
  for (const [file, source] of Object.entries(tree.tests)) {
    for (const specifier of importSpecifiers(withoutComments(source))) {
      const target = resolveSpecifier(file, specifier, names);
      if (target) follow(target, file);
    }
  }
  return new Map(
    [...naming].map(([module, files]) => [
      module,
      [...files].filter((file) => /\.test\.tsx?$/.test(file)).sort(),
    ]),
  );
}

/**
 * T-2728: the titles a Playwright spec declares, as the matcher of a rendered title.
 *
 * A literal title matches itself. A template title (`${kind}: created, …`) matches any rendering
 * of it, and `titleIn` then asks that every value standing in for a placeholder appears in the
 * spec as a string literal, so `Anything: created, …` is not a test because a helper exists.
 */
export function testTitles(source: string): RegExp[] {
  const title = /\btest(?:\.(?:only|skip|fixme|fail|slow))?\(\s*(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|`((?:[^`\\]|\\.)*)`)/g;
  return [...withoutComments(source).matchAll(title)].map(([, double, single, template]) => {
    const text = double ?? single ?? template;
    const parts = text.split(/\$\{[^}]*\}/).map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
    return new RegExp(`^${parts.join("(.+?)")}$`);
  });
}

/** The local modules a spec imports (`./kindJourney`), whose tests run under the spec's name. */
export function localImports(source: string): string[] {
  return importSpecifiers(withoutComments(source))
    .filter((specifier) => specifier.startsWith("./"))
    .map((specifier) => `${specifier.slice(2).replace(/\.ts$/, "")}.ts`);
}

/** Whether a spec, with the helpers it imports, declares a test of that rendered title. */
export function titleIn(title: string, spec: string, helpers: readonly string[]): boolean {
  const code = withoutComments(spec);
  return [spec, ...helpers].some((source) =>
    testTitles(source).some((matcher) => {
      const match = matcher.exec(title);
      if (!match) return false;
      return match
        .slice(1)
        .every((value) => code.includes(`"${value}"`) || code.includes(`'${value}'`) || code.includes(`\`${value}\``));
    }),
  );
}

/** Every mutating operation of an OpenAPI document, as `METHOD /path`. */
/** The entry of one registry operation (T-5948): the `/ops/{name}` door documented per name. */
const OPERATION_ENTRY = /^\/api\/v1\/projects\/\{project\}\/ops\/jc_[a-z_]+$/;

export function mutatingRoutes(openapi: { paths: Record<string, Record<string, unknown>> }): string[] {
  const routes = Object.entries(openapi.paths).flatMap(([path, operations]) =>
    Object.keys(operations)
      .filter((method) => ["post", "put", "patch", "delete"].includes(method))
      .map((method) => `${method.toUpperCase()} ${path.replace(OPERATION_ENTRY, "/api/v1/projects/{project}/ops/{name}")}`),
  );
  return [...new Set(routes)].sort();
}

/** One step of the workflow matrix, as `gate_workflows.test.ts` holds it to the tree. */
export interface CoverageStep {
  live?: { spec: string; test: string };
  liveOwed?: string;
  assistant?: string;
  assistantOwed?: string;
  /** Why no tool may take this step: only a person decides it (AG-11). */
  personOnly?: string;
  routes?: string[];
}

export interface Coverage {
  workflows: Record<string, Record<string, CoverageStep>>;
  unclaimed: Record<string, { why: string }>;
}

const STEP_HEADER = /^\|\s*step\s*\|\s*unit\s*\|\s*API\s*\|\s*mocked UI\s*\|\s*live journey\s*\|\s*assistant\s*\|$/;
const ROUTE_HEADER = /^\|\s*route\s*\|\s*steps\s*\|$/;

function cells(line: string): string[] {
  return line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim());
}

/**
 * T-2728: the matrix of `Testing/07-workflow-coverage.md`, read from its markdown.
 *
 * A workflow is the first backticked name of the heading above a step table; a step is the
 * backticked name in its first column. The live cell is `` `ui/e2e/live/x.spec.ts` › "title" ``
 * or `owed: T-nnnn`; the assistant cell is `` `jc_…` ``, `owed: T-nnnn` or `person only: why`.
 * The route table gives each mutating route its steps, or `none: why`. A row this cannot read
 * is thrown with its line, because a matrix read wrongly is a gate that bites nothing.
 */
export function parseCoverage(markdown: string): Coverage {
  const coverage: Coverage = { workflows: {}, unclaimed: {} };
  let workflow: string | undefined;
  let table: "steps" | "routes" | undefined;
  markdown.split("\n").forEach((raw, index) => {
    const line = raw.trim();
    const where = `line ${index + 1}`;
    if (line.startsWith("#")) {
      workflow = /`([a-z][a-z0-9-]*)`/.exec(line)?.[1];
      table = undefined;
      return;
    }
    if (STEP_HEADER.test(line)) {
      if (workflow === undefined) throw new Error(`${where}: a step table under a heading with no workflow name`);
      table = "steps";
      return;
    }
    if (ROUTE_HEADER.test(line)) {
      table = "routes";
      return;
    }
    if (!line.startsWith("|")) {
      table = undefined;
      return;
    }
    if (table === undefined || /^\|[\s|:-]+\|$/.test(line)) return;
    const row = cells(line);
    if (table === "routes") {
      const route = /^`((?:POST|PUT|PATCH|DELETE) \/\S+)`$/.exec(row[0])?.[1];
      if (route === undefined || row.length !== 2) throw new Error(`${where}: a route row reads \`METHOD /path\` | steps`);
      const none = /^none: (.+)$/.exec(row[1]);
      if (none) {
        coverage.unclaimed[route] = { why: none[1] };
        return;
      }
      const named = [...row[1].matchAll(/`([a-z][a-z0-9-]*)\/([a-z][a-z0-9-]*)`/g)];
      if (named.length === 0) throw new Error(`${where}: a route names its steps, or none: why`);
      for (const [, flow, step] of named) {
        const entry = coverage.workflows[flow]?.[step];
        if (entry === undefined) throw new Error(`${where}: ${flow}/${step} is no step of the matrix above`);
        entry.routes = [...(entry.routes ?? []), route];
      }
      return;
    }
    if (row.length !== 6 || workflow === undefined) throw new Error(`${where}: a step row has six cells`);
    const step = /^`([a-z][a-z0-9-]*)`$/.exec(row[0])?.[1];
    if (step === undefined) throw new Error(`${where}: the first cell is the step's name in backticks`);
    const entry: CoverageStep = {};
    const live = /^`ui\/e2e\/live\/([^`]+)` › "(.+)"$/.exec(row[4]);
    const liveOwed = /^owed: (T-\d{4})\b/.exec(row[4]);
    if (live) entry.live = { spec: live[1], test: live[2] };
    else if (liveOwed) entry.liveOwed = liveOwed[1];
    else throw new Error(`${where}: the live cell is \`ui/e2e/live/…\` › "title" or owed: T-nnnn`);
    const tool = /^`([a-z_]+)`/.exec(row[5]);
    const toolOwed = /^owed: (T-\d{4})\b/.exec(row[5]);
    const person = /^person only: (.+)$/.exec(row[5]);
    if (tool) entry.assistant = tool[1];
    else if (toolOwed) entry.assistantOwed = toolOwed[1];
    else if (person) entry.personOnly = person[1];
    else throw new Error(`${where}: the assistant cell is a tool, owed: T-nnnn or person only: why`);
    coverage.workflows[workflow] ??= {};
    coverage.workflows[workflow][step] = entry;
  });
  return coverage;
}
