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

function frameOf(name, values) {
  const entity = { NaN: [], Inf: [] };
  const plain = values.map((value, index) => {
    if (value === "NaN" || value === "Inf") {
      entity[value].push(index);
      return null;
    }
    return value;
  });
  const entities = Object.fromEntries(Object.entries(entity).filter(([, list]) => list.length));
  return {
    schema: { name, refId: "A", fields: [{ name, type: "number" }] },
    data: { values: [plain], ...(Object.keys(entities).length ? { entities: [entities] } : {}) },
  };
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
  panels: [
    {
      id: 1,
      type: "table",
      title: "Every reducer",
      gridPos: { x: 0, y: 0, w: 24, h: 20 },
      datasource: { type: "grafana-testdata-datasource", uid: "testdata" },
      targets: [{ refId: "A", scenarioId: "raw_frame", datasource: { type: "grafana-testdata-datasource", uid: "testdata" }, rawFrameContent: JSON.stringify(Object.entries(CASES).map(([name, values]) => frameOf(name, values))) }],
      transformations: [{ id: "reduce", options: { mode: "seriesToRows", reducers, includeTimeField: false } }],
    },
  ],
};
writeFileSync(join(REF, "parity-calcs.json"), JSON.stringify(dashboard, null, 1));
// Grafana reads its provisioned files every 10 s.
await new Promise((resolve) => setTimeout(resolve, 15_000));

const browser = await chromium.launch();
const context = await browser.newContext({ acceptDownloads: true, viewport: { width: 1440, height: 1000 } });
const page = await context.newPage();
await page.goto(`${GRAFANA}/login`);
await page.getByTestId("data-testid Username input field").fill("admin");
await page.getByTestId("data-testid Password input field").fill("admin");
await page.getByTestId("data-testid Login button").click();
// The reference keeps Grafana's first-login password: skip the change it offers.
const skip = page.getByRole("button", { name: "Skip" });
await skip.click({ timeout: 10_000 }).catch(() => undefined);
await page.waitForURL((url) => !url.pathname.startsWith("/login"));
await page.goto(`${GRAFANA}/d/parity-calcs?orgId=1&inspect=panel-1&inspectTab=data`, { waitUntil: "networkidle" });
await page.getByText("Data options").click();
// The raw values, not the display text: "Formatted data" off, the transformation applied.
const formatted = page.getByLabel("Formatted data");
if (await formatted.isChecked()) await formatted.click({ force: true });
const download = page.waitForEvent("download");
await page.getByRole("button", { name: /Download CSV/ }).click();
const csv = readFileSync(await (await download).path(), "utf8");
await browser.close();

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

const lines = csv.replace(/^﻿/, "").trim().split(/\r?\n/);
const head = cells(lines[0]);
const values = Object.fromEntries(
  lines.slice(1).map((line) => {
    const row = cells(line);
    return [row[0], Object.fromEntries(head.slice(1).map((name, i) => [name, row[i + 1]]))];
  }),
);
writeFileSync(OUT, `${JSON.stringify({ grafana: "12.4.14", reducers, head, cases: CASES, values }, null, 1)}\n`);
console.log(`${reducers.length} reducers, ${Object.keys(values).length} cases`);
