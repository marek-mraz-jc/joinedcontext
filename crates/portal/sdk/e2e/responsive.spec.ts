import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import react from "@vitejs/plugin-react";
import { build } from "vite";
import { BLOCKS, LIVE_BLOCKS, WIDTHS, layoutProblems } from "../src/sdk/responsive";
import { sampleOverlay, sdkAlias } from "../vite.config";

/**
 * Every view of an application at a phone, a tablet, a laptop and a wall (T-2777, UI-84, SDK-12).
 *
 * The template's own screens (the overview and a type page: filters, tiles, map, charts, table,
 * export) and a page built from each layout primitive (sidebar, tabs, split with a form and a
 * detail, a grid of cards, the NGSI-LD grid) are built with vite from the sources and served from
 * memory. At each width the page must not scroll sideways, no two blocks may overlap, and axe
 * must find nothing at WCAG 2.1 AA. A screenshot per view and width is attached to the report.
 *
 * The same three checks run against published applications when `JC_APP_URLS` names them
 * (space-separated), for the integrator after an apply:
 *
 *   JC_APP_URLS="https://dev.joinedcontext.com/apps/helsinki-bikes/ …" \
 *     [JC_STORAGE_STATE=signed-in.json] pnpm exec playwright test e2e/responsive.spec.ts -g live
 *
 * `JC_STORAGE_STATE` is a Playwright storage state of a signed-in person, for an App behind the
 * login; a public App needs none.
 */
const PAGE = "http://responsive.test/";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import "../../template/map-worker.ts";
import "../../src/sdk/style.css";
import "../../template/src/components/components.css";
import "../../template/src/app.css";
import { applyTokens, Card, EntityGrid, fixtureSource, Grid, Header, JcProvider, Page, Sidebar, Split, Tabs } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../../template/src/App.tsx";
import { EntityDetail } from "../../template/src/components/EntityDetail.tsx";
import { EntityForm } from "../../template/src/components/EntityForm.tsx";
import { EntityTable } from "../../samples/operations-desk/src/components/EntityTable.tsx";
import { StatTiles } from "../../template/src/components/StatTiles.tsx";
import tokens from "../../template/src/design-tokens.json";
import schema from "../../tests/fixtures/helsinki-air-quality.schema.json";

const STATIONS = ["Kallio", "Mäkelänkatu", "Leppävaara", "Kamppi", "Vallila", "Tikkurila", "Espoonlahti", "Malmi"];
const ROWS = Array.from({ length: 24 }, (_, i) => ({
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:" + (i + 1),
  type: "AirQualityObserved",
  stationName: STATIONS[i % STATIONS.length],
  pm25: Math.round((4 + ((i * 7) % 11) + i / 10) * 10) / 10,
  temperature: 10 + (i % 9),
  dateObserved: new Date(Date.UTC(2026, 8, 1 + i, 8)).toISOString(),
  refStation: "urn:ngsi-ld:Station:hel.fi:air-quality:" + ((i % 8) + 1),
  location: { type: "Point", coordinates: [24.8 + (i % 8) * 0.03, 60.15 + (i % 5) * 0.02] },
}));
const ALL = { permissions: [{ actions: ["*"], resource: { type: "*" }, attributes: "*" }], prohibitions: [] };
const client = stubClient({
  entities: ROWS,
  schema: schema.definitions,
  access: ALL,
  functions: { summary: () => ({ types: [{ type: "AirQualityObserved", count: ROWS.length, averages: { pm25: 9.1 } }] }) },
}, { appName: "Air quality", endpointName: "helsinki-air-quality" });

const GRID = {
  source: { kind: "fixture", name: "readings" },
  type: "AirQualityObserved",
  columns: [{ attr: "stationName" }, { attr: "pm25" }, { attr: "temperature" }, { attr: "dateObserved" }, { attr: "refStation" }],
  entityTimestamps: false, filters: {}, pageSize: 10, mode: "view", editableAttrs: [],
  history: { enabled: false }, density: "comfortable", rowActions: [],
};
const NGSI = ROWS.map((row) => ({
  id: row.id, type: row.type,
  stationName: { type: "Property", value: row.stationName },
  pm25: { type: "Property", value: row.pm25, unitCode: "GQ" },
  // Vallila reports no temperature: its empty cell carries an absolute screen-reader "not
  // set" span, which once escaped the scroll box and widened a phone page by 565 px (T-6323).
  ...(row.stationName === "Vallila" ? {} : { temperature: { type: "Property", value: row.temperature, unitCode: "CEL" } }),
  dateObserved: { type: "Property", value: row.dateObserved },
  refStation: { type: "Relationship", object: row.refStation },
}));

function Layout() {
  const tiles = [{ label: "Readings", agg: "count" }, { label: "Average pm25", agg: "avg", attr: "pm25" }];
  return h("div", { className: "jc-shell" },
    h("main", { className: "jc-main" },
      h(Page, { label: "Desk" },
        h(Header, { level: 1, title: "Air quality desk", subtitle: "Readings of the last month", actions: h("button", { type: "button" }, "Export") }),
        h(Sidebar, { label: "Filters", side: h("p", null, "Stations and dates") },
          h(Tabs, { label: "Views of the readings", tabs: [
            { id: "table", label: "Table", render: () => h(EntityTable, { rows: ROWS, caption: "Readings", pageSize: 8 }) },
            { id: "detail", label: "Detail", render: () => h(EntityDetail, { row: ROWS[0], title: ROWS[0].stationName }) },
          ] }),
          h(Split, { ratio: "2:1" },
            h(EntityForm, { type: "AirQualityObserved", row: ROWS[1], rows: ROWS }),
            h(EntityDetail, { row: ROWS[1], title: ROWS[1].stationName }),
          ),
          h(Grid, { columns: 4 },
            ...STATIONS.slice(0, 4).map((name) =>
              h(Card, { key: name, title: name }, h(StatTiles, { rows: ROWS.filter((row) => row.stationName === name), tiles })),
            ),
          ),
          h(Card, { title: "All readings" }, h(EntityGrid, { config: GRID, source: fixtureSource(NGSI) })),
        ),
      ),
    ),
  );
}

// The gallery (T-2778): each sample over the template, with its own look, stylesheet and rows.
const SAMPLES = import.meta.glob("../../samples/*/src/App.tsx");
const SAMPLE_ROWS = import.meta.glob("../../samples/*/src/fixtures.ts");
const SAMPLE_CSS = import.meta.glob("../../samples/*/src/app.css");
const SAMPLE_TOKENS = import.meta.glob("../../samples/*/src/design-tokens.json", { eager: true, import: "default" });

const params = new URLSearchParams(location.search);
const view = params.get("view");
const root = createRoot(document.getElementById("root"));
if (view === "sample") {
  const at = "../../samples/" + params.get("name") + "/src/";
  const [{ default: Sample }, fixtures] = await Promise.all([SAMPLES[at + "App.tsx"](), SAMPLE_ROWS[at + "fixtures.ts"](), SAMPLE_CSS[at + "app.css"]?.()]);
  applyTokens(SAMPLE_TOKENS[at + "design-tokens.json"]);
  const sampleClient = stubClient({
    entities: fixtures.ROWS, schema: fixtures.SCHEMA, access: ALL, functions: fixtures.FUNCTIONS, temporal: fixtures.TEMPORAL, services: fixtures.SERVICES,
  }, { appName: params.get("name"), endpointName: "sample", user: fixtures.USER ?? null });
  root.render(h(JcProvider, { client: sampleClient }, h(Sample)));
} else {
  applyTokens(tokens);
  root.render(h(JcProvider, { client }, view === "layout" ? h(Layout) : h(App)));
}
`;

const HTML = `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Responsive check</title>
</head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

const files: Record<string, string> = {};

async function buildFixture(): Promise<void> {
  const here = dirname(fileURLToPath(import.meta.url));
  // Inside the package, not in /tmp: the entry imports `../src` and needs this `node_modules`.
  const work = join(here, "..", "node_modules", ".responsive-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({
    root: work,
    logLevel: "silent",
    configFile: false,
    plugins: [react(), sampleOverlay],
    resolve: { alias: sdkAlias },
    build: { outDir: out, emptyOutDir: true, target: "es2022", assetsInlineLimit: 64 * 1024 },
  });
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        walk(join(dir, entry.name), `${prefix}${entry.name}/`);
      } else {
        files[`/${prefix}${entry.name}`] = readFileSync(join(dir, entry.name), "utf8");
      }
    }
  };
  walk(out, "");
  files["/"] = files["/index.html"];
}

async function serve(page: Page, problems: string[]): Promise<void> {
  page.on("pageerror", (error) => problems.push(error.message));
  await page.route(`${PAGE}**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    const body = files[path];
    if (body === undefined) {
      return route.fulfill({ status: 404, body: "" });
    }
    return route.fulfill({
      contentType: path.endsWith(".css") ? "text/css" : path === "/" ? "text/html" : "text/javascript",
      body,
    });
  });
}

/** The screenshot attached first, then everything `layoutProblems` finds at this width. */
async function checkWidth(page: Page, name: string, width: number, blocks: string, testInfo: TestInfo): Promise<void> {
  await testInfo.attach(`${name}-${width}.png`, {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(page, blocks)).toEqual([]);
}

const SAMPLES_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "samples");
const VIEWS = [
  { name: "overview", path: "#/overview", ready: "Server summary" },
  { name: "type page", path: "#/AirQualityObserved", ready: "pm25 over time" },
  { name: "layout primitives", path: "?view=layout", ready: "All readings" },
  // Every sample of the gallery (T-2778), ready when its title is on screen.
  ...readdirSync(SAMPLES_DIR, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => ({
      name: `sample ${entry.name}`,
      path: `?view=sample&name=${entry.name}`,
      ready: (JSON.parse(readFileSync(join(SAMPLES_DIR, entry.name, "sample.json"), "utf8")) as { ready: string }).ready,
    })),
];

test.describe("the template's views", () => {
  test.beforeAll(buildFixture);
  for (const view of VIEWS) {
    for (const size of WIDTHS) {
      test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        const problems: string[] = [];
        await serve(page, problems);
        await page.setViewportSize(size);
        await page.goto(`${PAGE}${view.path}`);
        await expect(page.getByText(view.ready).first()).toBeVisible();
        // The charts draw after the rows, in an effect: wait for their canvases before measuring.
        const charts = page.locator(".jc-chart-canvas");
        for (let i = 0; i < (await charts.count()); i++) {
          await expect(charts.nth(i).locator("canvas")).toBeVisible();
        }
        await checkWidth(page, view.name, size.width, BLOCKS, testInfo);
        expect(problems).toEqual([]);
      });
    }
  }
});

// The default type page's grid (T-6437): what each width owes a person, measured in the browser.
test.describe("the type page's grid", () => {
  test.beforeAll(buildFixture);
  const open = async (page: Page, width: number) => {
    const problems: string[] = [];
    await serve(page, problems);
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${PAGE}#/AirQualityObserved`);
    await expect(page.locator(".jc-type-grid tbody td").first()).toBeVisible();
  };

  test("at 375 px the header is one line and a row shows every value without a swipe", async ({ page }) => {
    await open(page, 375);
    const head = page.locator(".jc-type-grid thead");
    expect((await head.boundingBox())?.height).toBeLessThan(60);
    const row = page.locator(".jc-type-grid tbody tr.jc-grid-tr").first();
    const cells = await row.locator("td").evaluateAll((all) =>
      all.map((cell) => ({ right: cell.getBoundingClientRect().right, clipped: cell.scrollWidth > cell.clientWidth, label: cell.getAttribute("data-label") })),
    );
    expect(cells.map((cell) => cell.label)).toEqual(["Station name", "pm25", "Temperature", "Date observed"]);
    for (const cell of cells) {
      expect(cell.right).toBeLessThanOrEqual(375);
      expect(cell.clipped).toBe(false);
    }
    const scroll = page.locator(".jc-type-grid .jc-grid-scroll");
    expect(await scroll.evaluate((box) => box.scrollWidth - box.clientWidth)).toBe(0);
    await expect(page.getByText("Swipe sideways")).toHaveCount(0);
  });

  test("at 375 px a column's menu opens whole, beyond the sliding header", async ({ page }) => {
    await open(page, 375);
    await page.getByRole("button", { name: "Column options: pm25" }).click();
    const menu = page.locator(".jc-grid-meta-menu");
    await expect(menu).toBeVisible();
    const box = await menu.boundingBox();
    expect(box?.x).toBeGreaterThanOrEqual(0);
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(375);
    expect(box?.height).toBeGreaterThan(100);
  });

  test("at 768 px the last column ends inside the grid", async ({ page }) => {
    await open(page, 768);
    const scroll = page.locator(".jc-type-grid .jc-grid-scroll");
    expect(await scroll.evaluate((box) => box.scrollWidth - box.clientWidth)).toBe(0);
  });

  test("at 1440 px a number sits at its cell's end, in tabular figures", async ({ page }) => {
    await open(page, 1440);
    const number = page.locator('.jc-type-grid tbody td[data-kind="number"]').first();
    const style = await number.locator(".jc-grid-cell").evaluate((cell) => {
      const { textAlign, fontVariantNumeric } = getComputedStyle(cell);
      return { textAlign, fontVariantNumeric };
    });
    expect(style).toEqual({ textAlign: "end", fontVariantNumeric: "tabular-nums" });
    await expect(page.locator(".jc-type-grid tbody td.jc-grid-pinned .jc-grid-cell").first()).not.toHaveCSS("text-align", "end");
  });
});

const LIVE = (process.env.JC_APP_URLS ?? "").split(/\s+/).filter(Boolean);

test.describe("live", () => {
  if (process.env.JC_STORAGE_STATE) {
    test.use({ storageState: process.env.JC_STORAGE_STATE });
  }
  for (const url of LIVE) {
    for (const size of WIDTHS) {
      test(`live ${url} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.setViewportSize(size);
        const answer = await page.goto(url);
        expect(answer?.status(), `${url} answers`).toBeLessThan(400);
        await page.waitForLoadState("networkidle");
        await checkWidth(page, new URL(url).pathname.replace(/\W+/g, "-"), size.width, LIVE_BLOCKS, testInfo);
      });
    }
  }
});

// T-2981: a table inside a closed <details> is never drawn, yet Chromium lays it out when asked
// for its box, so the check counted it over the card below and turned bbsk-zaznamy red at 375 px.
test.describe("the check itself", () => {
  const PAGE_WITH = (open: boolean) => `<!doctype html><html lang="en"><head><title>The check</title></head><body>
    <details${open ? " open" : ""}><summary>Values</summary>
      <table style="position:absolute;top:40px;left:0;width:200px;height:100px"><tr><th>2023</th><td>1 893</td></tr></table>
    </details>
    <figure style="position:absolute;top:40px;left:0;width:200px;height:100px;margin:0">A chart</figure>
  </body></html>`;

  test("a closed <details> hides its content from the overlap check", async ({ page }) => {
    await page.setContent(PAGE_WITH(false));
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
  });

  // T-2827: a chart's table of values is kept for a screen reader, clipped to nothing; a table
  // never shrinks to its 1 px, so its box stood over the card it is in.
  test("a block clipped away for a screen reader is not counted, a visible one is", async ({ page }) => {
    const card = `<article style="position:absolute;top:0;left:0;width:300px;height:200px">A card</article>`;
    const SR = "position:absolute;width:1px;height:1px;margin:-1px;overflow:hidden;clip:rect(0, 0, 0, 0);white-space:nowrap";
    await page.setContent(`<!doctype html><html lang="en"><head><title>The check</title></head><body>${card}
      <table style="${SR};top:20px;left:20px"><tr><th>Category</th><td>closed 7</td><td>open 2</td></tr></table></body></html>`);
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
    await page.setContent(`<!doctype html><html lang="en"><head><title>The check</title></head><body>${card}
      <table style="position:absolute;top:20px;left:20px"><tr><th>Category</th><td>closed 7</td><td>open 2</td></tr></table></body></html>`);
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([expect.stringMatching(/^overlap: article\. .* × table\./)]);
  });

  // T-2827: a fixed 900 px table scrolls the page sideways on a phone and fits on a laptop.
  test("a fixed 900 px table fails at 375 px and passes at 1440 px", async ({ page }) => {
    const html = `<!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width, initial-scale=1"><title>The check</title></head>
      <body style="margin:0"><main><table style="width:900px"><caption>Readings</caption><tr><th scope="col">Station</th><th scope="col">pm25</th></tr><tr><td>Kallio</td><td>9.1</td></tr></table></main></body></html>`;
    await page.setViewportSize(WIDTHS[0]);
    await page.setContent(html);
    expect((await layoutProblems(page, LIVE_BLOCKS)).length).toBeGreaterThan(0);
    await page.setViewportSize(WIDTHS[2]);
    await page.setContent(html);
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
  });

  test("the same content open is counted", async ({ page }) => {
    await page.setContent(PAGE_WITH(true));
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([expect.stringMatching(/^overlap: table\. .* × figure\./)]);
  });
});
