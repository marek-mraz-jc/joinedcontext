// Grafana 12.4.14's reducers, read from the reference instance itself: the ReducerID list from its
// own bundle, then every reducer over the edge-case fields below as Grafana computes them (a
// table panel with the Reduce transformation over TestData frames, exported by the panel
// inspector as CSV, unformatted). Writes tests/fixtures/grafana-reference/calcs.json, which
// tests/dashboard_calcs.test.ts compares with frames.ts `reduce`, value by value.
//
//   GRAFANA=http://localhost:42161 REF=<provisioned dashboards dir> node scripts/grafana-calcs.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const here = dirname(fileURLToPath(import.meta.url));
const GRAFANA = process.env.GRAFANA ?? "http://localhost:42161";
const REF = process.env.REF;
if (!REF) throw new Error("set REF to the directory the reference Grafana provisions dashboards from");
const OUT = join(here, "../tests/fixtures/grafana-reference/calcs.json");

/** The edge cases: each a number field; `NaN` and `Inf` travel as DataFrameJSON entities. */
export const CASES = {
  typical: [3, 1, 4, 1, 5, 9, 2, 6],
  nulls: [null, 2, null, 7, 3, null],
  nan: [1, "NaN", 5],
  inf: [1, "Inf", 5],
  single: [42],
  empty: [],
  allNull: [null, null],
  zeros: [0, 0, 0],
  negative: [-3, -1, -7],
  counterReset: [10, 20, 30, 5, 15, 25],
  resetOnLast: [10, 20, 30, 5],
  decreasing: [9, 7, 4],
  repeated: [1, 1, 2, 2, 2, 3],
  fractions: [0.1, 0.2, 0.3],
  mixed: [1, "2", true, null, 4],
};

/**
 * A case as TestData's backend `csv_content` scenario reads it, so nulls arrive as a backend's
 * nulls do (Arrow nulls; the frontend `raw_frame` scenario turns a JSON null into NaN): an `i`
 * column keeps each row, an empty cell is a null, `NaN` and `Inf` are Go's float words.
 */
function csvOf(name, values) {
  return [`i,${name}`, ...values.map((value, index) => `${index},${value === null ? "" : value === "Inf" ? "+Inf" : value}`)].join("\n");
}

async function reducerIds() {
  const login = await (await fetch(`${GRAFANA}/login`)).text();
  for (const script of new Set(login.match(/public\/build\/[^"]+\.js/g))) {
    const text = await (await fetch(`${GRAFANA}/${script}`)).text();
    const found = /\(([a-zA-Z$_]+)\.sum="sum",(.*?),\1\)\)\(/.exec(text);
    if (found) return ["sum", ...[...found[2].matchAll(/\.([a-zA-Z0-9]+)="\1"/g)].map((m) => m[1])];
  }
  throw new Error("no ReducerID enum in the reference bundle");
}

const reducers = await reducerIds();
const dashboard = {
  uid: "parity-calcs",
  title: "Parity: every reducer",
  schemaVersion: 41,
  time: { from: "now-6h", to: "now" },
  // One panel per case: Grafana's Reduce output leaves out a reducer's undefined results, which
  // shifts that column's later rows when several fields share one table.
  panels: Object.entries(CASES).map(([name, values], index) => ({
    id: index + 1,
    type: "table",
    title: name,
    gridPos: { x: 0, y: index * 4, w: 24, h: 4 },
    datasource: { type: "grafana-testdata-datasource", uid: "testdata" },
    targets: [{ refId: "A", scenarioId: "csv_content", datasource: { type: "grafana-testdata-datasource", uid: "testdata" }, csvContent: csvOf(name, values) }],
    transformations: [{ id: "reduce", options: { mode: "seriesToRows", reducers, includeTimeField: false } }],
  })),
};
writeFileSync(join(REF, "parity-calcs.json"), JSON.stringify(dashboard, null, 1));
// Grafana reads its provisioned files every 10 s.
await new Promise((resolve) => setTimeout(resolve, 15_000));

const browser = await chromium.launch();
const context = await browser.newContext({ acceptDownloads: true, viewport: { width: 1440, height: 6000 } });
const page = await context.newPage();
await page.goto(`${GRAFANA}/login`);
await page.getByTestId("data-testid Username input field").fill("admin");
await page.getByTestId("data-testid Password input field").fill("admin");
await page.getByTestId("data-testid Login button").click();
// The reference keeps Grafana's first-login password: skip the change it offers.
const skip = page.getByRole("button", { name: "Skip" });
await skip.click({ timeout: 10_000 }).catch(() => undefined);
await page.waitForURL((url) => !url.pathname.startsWith("/login"));
/** Opens the inspector of panel `title` on `tab` (`Data`, `Panel JSON`). */
async function inspect(title, tab) {
  await page.goto(`${GRAFANA}/d/parity-calcs?orgId=1`, { waitUntil: "networkidle" });
  const panel = page.locator(`[data-viz-panel-key="panel-${Object.keys(CASES).indexOf(title) + 1}"]`);
  await panel.scrollIntoViewIfNeeded();
  await panel.hover({ position: { x: 300, y: 10 } });
  await page.getByTestId(`data-testid Panel menu ${title}`).click();
  await page.getByRole("menuitem", { name: /^Inspect/ }).first().hover();
  await page.getByRole("menuitem", { name: new RegExp(`^${tab}`) }).first().click();
}

const raw = {};
for (const title of Object.keys(CASES)) {
  // The CSV names NaN and Infinity, which JSON writes as null.
  await inspect(title, "Data");
  await page.getByRole("button", { name: "Expand query row" }).click();
  // Transformations on, formatted data off (the drawer's switches: transformations, formatted, Excel).
  const switches = page.getByRole("dialog").locator("input[type=checkbox]");
  await switches.first().waitFor({ state: "attached" });
  if (!(await switches.nth(0).isChecked())) await switches.nth(0).click({ force: true });
  if (await switches.nth(1).isChecked()) await switches.nth(1).click({ force: true });
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: /Download CSV/ }).click();
  const csv = readFileSync(await (await download).path(), "utf8");
  // The panel data tells null from a missing (undefined) result.
  await inspect(title, "Panel JSON");
  await page.getByRole("dialog").locator("[role=combobox]").first().click();
  await page.getByRole("option", { name: /Panel data/ }).click();
  await page.waitForTimeout(800);
  const json = await page.evaluate(() => window.monaco.editor.getModels().map((m) => m.getValue()).sort((a, b) => b.length - a.length)[0]);
  raw[title] = { csv, data: JSON.parse(json).series[0].fields.map((f) => ({ name: f.name, values: f.values })) };
}
await browser.close();
if (process.env.RAW) writeFileSync(process.env.RAW, JSON.stringify(raw, null, 1));

/** One CSV line as its cells (Grafana quotes every cell that holds a comma or a quote). */
function cells(line) {
  const out = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < line.length; i += 1) {
    const c = line[i];
    if (quoted && c === '"' && line[i + 1] === '"') {
      cell += '"';
      i += 1;
    } else if (c === '"') quoted = !quoted;
    else if (c === "," && !quoted) {
      out.push(cell);
      cell = "";
    } else cell += c;
  }
  out.push(cell);
  return out;
}

/**
 * Each reducer's result per case: a number, a boolean, an array, `null` (null or NaN),
 * `"Infinity"`, `"-Infinity"`, or `"undefined"` where Grafana's reducer gives nothing.
 */
const values = {};
for (const [title, { csv, data }] of Object.entries(raw)) {
  const lines = csv.replace(/^\uFEFF/, "").trim().split(/\r?\n/);
  const head = cells(lines[0]);
  // One row per field of the case's frame (`i` and the case): take the case's own row.
  const at = data[0].values.indexOf(title);
  const row = at >= 0 && lines[at + 1] ? cells(lines[at + 1]) : [];
  values[title] = {};
  for (const field of data.slice(1)) {
    const text = row[head.indexOf(field.name)];
    const value = at >= 0 && at < field.values.length ? field.values[at] : "undefined";
    // JSON writes NaN, Infinity, null and an undefined result alike as null, and the CSV writes
    // NaN for both NaN and null: an infinity and an undefined result are told apart by their CSV
    // text. A null here is null or NaN.
    values[title][field.name] = value === null && ["Infinity", "-Infinity", "undefined"].includes(text) ? text : value;
  }
}
const names = raw[Object.keys(raw)[0]].data.slice(1).map((f) => f.name);
writeFileSync(OUT, `${JSON.stringify({ grafana: "12.4.14", reducers, names, cases: CASES, values }, null, 1)}\n`);
console.log(`${reducers.length} reducers, ${Object.keys(values).length} cases`);
