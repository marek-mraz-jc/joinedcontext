// The Apps' coverage gate (T-3373): every App of `apps/` and every SDK example near 100 % tested.
//
//   node scripts/app-coverage-gate.mjs app <name> [--coverage <coverage-summary.json>] [--controls <dir>] [--rust <llvm-cov.json>]
//   node scripts/app-coverage-gate.mjs pending <base-pending.txt>
//
// `app` holds one App to the thresholds of apps/coverage-thresholds.json: vitest's v8 summary
// (lines, statements, functions, branches), the controls record of the SDK's `recordControls`
// (every rendered control exercised by a test) and `cargo llvm-cov --json` of its Rust part. An App
// listed in apps/coverage-pending.txt is measured and reported, not failed, until its own task
// brings it to green; a listed App that already passes fails, so its task removes the line.
// `pending` fails when the list gained a line against the base, so the list only shrinks.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

/** The thresholds, the one shared config; never lowered for one App. */
export function thresholds(root = ROOT) {
  return JSON.parse(readFileSync(join(root, "apps/coverage-thresholds.json"), "utf8"));
}

/** `name task` per line, `#` comments and blank lines ignored. */
export function parsePending(text) {
  const pending = new Map();
  for (const raw of text.split("\n")) {
    const line = raw.replace(/#.*/, "").trim();
    if (line === "") continue;
    const [name, task, ...rest] = line.split(/\s+/);
    if (!/^[a-z0-9][a-z0-9-]*$/.test(name) || !/^T-\d+$/.test(task ?? "") || rest.length > 0) {
      throw new Error(`apps/coverage-pending.txt: "${raw}" is not "<app> <T-id>"`);
    }
    pending.set(name, task);
  }
  return pending;
}

/** The lines the current list has that the base did not: what the gate refuses. */
export function addedLines(base, current) {
  const before = parsePending(base);
  return [...parsePending(current)].filter(([name]) => !before.has(name)).map(([name, task]) => `${name} ${task}`);
}

/** What one App falls short of; empty when it meets every threshold it has a report for. */
export function shortfalls({ summary, controls, rust }, limits) {
  const found = [];
  if (summary === undefined) {
    found.push("no coverage report: run vitest with --coverage.reporter=json-summary");
  } else {
    for (const metric of ["lines", "statements", "functions", "branches"]) {
      const pct = summary.total?.[metric]?.pct;
      if (typeof pct !== "number" || pct < limits[metric]) found.push(`${metric} ${pct ?? "?"} % < ${limits[metric]} %`);
    }
  }
  if (controls === undefined) {
    found.push("no controls record: call recordControls(afterAll) in the test setup");
  } else {
    for (const control of controls.untouched) found.push(`control no test exercises: ${control}`);
  }
  if (rust !== undefined) {
    const pct = rust.data?.[0]?.totals?.lines?.percent;
    if (typeof pct !== "number" || pct < limits.rustLines) found.push(`rust lines ${pct === undefined ? "?" : pct.toFixed(1)} % < ${limits.rustLines} %`);
  }
  return found;
}

/** The union of the controls records a test run wrote, one file per test file. */
export function readControls(dir) {
  if (!dir || !existsSync(dir)) return undefined;
  const files = readdirSync(dir).filter((file) => file.endsWith(".json"));
  if (files.length === 0) return undefined;
  const rendered = new Set();
  const exercised = new Set();
  for (const file of files) {
    const record = JSON.parse(readFileSync(join(dir, file), "utf8"));
    for (const id of record.rendered ?? []) rendered.add(id);
    for (const id of record.exercised ?? []) exercised.add(id);
  }
  return { untouched: [...rendered].filter((id) => !exercised.has(id)).sort() };
}

/** The verdict for one App: what it lacks, and whether that fails the gate. */
export function verdict(name, reports, pending, limits) {
  const lacking = shortfalls(reports, limits);
  const task = pending.get(name);
  if (task === undefined) return { ok: lacking.length === 0, lines: lacking };
  if (lacking.length === 0) return { ok: false, lines: [`${name} passes every threshold: remove its line from apps/coverage-pending.txt (${task})`] };
  return { ok: true, lines: lacking.map((line) => `pending (${task}): ${line}`) };
}

function read(path) {
  return path && existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : undefined;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [mode, ...rest] = process.argv.slice(2);
  const option = (flag) => {
    const at = rest.indexOf(flag);
    return at >= 0 ? rest[at + 1] : undefined;
  };
  const pendingText = readFileSync(join(ROOT, "apps/coverage-pending.txt"), "utf8");
  if (mode === "pending") {
    // No base list (the commit that introduces it): nothing to compare, the rest still holds.
    const base = rest[0] && existsSync(rest[0]) ? readFileSync(rest[0], "utf8") : undefined;
    const added = base === undefined ? [] : addedLines(base, pendingText);
    for (const line of added) console.error(`apps/coverage-pending.txt only shrinks; this line is new: ${line}`);
    // A line for an App that is no longer there would wait for a task that has nothing to do.
    const gone = [...parsePending(pendingText).keys()].filter((name) => !existsSync(join(ROOT, "apps", name)) && !existsSync(join(ROOT, "sdk/examples", name)));
    for (const name of gone) console.error(`apps/coverage-pending.txt names ${name}, which is no App of apps/ or sdk/examples/`);
    process.exit(added.length > 0 || gone.length > 0 ? 1 : 0);
  }
  if (mode !== "app" || !rest[0]) {
    console.error("usage: app-coverage-gate.mjs app <name> [--coverage f] [--controls dir] [--rust f] | pending <base>");
    process.exit(2);
  }
  const name = rest[0];
  const reports = { summary: read(option("--coverage")), controls: readControls(option("--controls")), rust: read(option("--rust")) };
  const result = verdict(name, reports, parsePending(pendingText), thresholds());
  for (const line of result.lines) console.log(`${name}: ${line}`);
  if (result.ok && result.lines.length === 0) console.log(`${name}: every threshold met`);
  process.exit(result.ok ? 0 : 1);
}
