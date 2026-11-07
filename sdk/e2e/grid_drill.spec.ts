import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * The grid's review of a GeoProperty and a Relationship change in a real browser (T-5530): the row
 * by its name, the column by its label, each value drawn as the grid's own cell, and the URN only
 * behind the copy button. Built from the sources with vite and served from memory, offline.
 */
const PAGE = "http://griddrill.test/";
const SHOTS = process.env.SHOTS_DIR ?? "test-results/grid-drill";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource } from "../../src/grid/source";

const p = (value, extra) => ({ type: "Property", value, ...(extra || {}) });
const COMPANIES = [
  { id: "urn:ngsi-ld:Company:hsl", type: "Company", name: p("Helsinki City Transport"), employees: p(1400), founded: p("2010-01-01") },
  { id: "urn:ngsi-ld:Company:cityBikes", type: "Company", name: p("City Bikes Oy"), employees: p(85), founded: p("2016-05-02") },
  { id: "urn:ngsi-ld:Company:closed", type: "Company", name: p("Hidden Owner Ltd"), employees: p(3) },
];
const HIDDEN = ["urn:ngsi-ld:Company:closed"];
const STATIONS = [
  { id: "urn:ngsi-ld:Station:kamppi", type: "Station", name: p("Kamppi"), status: p("working"), bikes: [p(12), p(11, { datasetId: "urn:ngsi-ld:Dataset:sensor" }), p(13, { datasetId: "urn:ngsi-ld:Dataset:camera" })], owner: { type: "Relationship", object: "urn:ngsi-ld:Company:hsl" } },
  { id: "urn:ngsi-ld:Station:kallio", type: "Station", name: { type: "LanguageProperty", languageMap: { fi: "Kallion kirjasto" } }, status: p("closed"), bikes: p("n/a"), owner: { type: "Relationship", object: "urn:ngsi-ld:Company:cityBikes" } },
  { id: "urn:ngsi-ld:Station:pasila", type: "Station", name: p("Pasila"), status: p("zzz"), bikes: p(null), owner: { type: "Relationship", object: "urn:ngsi-ld:Company:closed" } },
  { id: "urn:ngsi-ld:Station:toolo", type: "Station", name: p("Töölö"), status: p("working"), bikes: p(4), partners: { type: "ListRelationship", objectList: [{ object: "urn:ngsi-ld:Company:hsl" }, { object: "urn:ngsi-ld:Company:cityBikes" }] } },
];
const fixture = fixtureSource([...STATIONS, ...COMPANIES]);
// The person's grants do not reach one company: the source never answers it.
const source = { ...fixture, query: async (q, page) => { const a = await fixture.query(q, page); return { ...a, rows: a.rows.filter((r) => !HIDDEN.includes(r.id)) }; } };
const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "Station",
  columns: [{ attr: "name", label: "Name" }, { attr: "status", label: "Status" }, { attr: "bikes", label: "Bikes" }, { attr: "owner", label: "Owner" }, { attr: "partners", label: "Partners" }],
  entityTimestamps: false, filters: {}, pageSize: 50, mode: "view", editableAttrs: [], history: { enabled: false },
  density: "comfortable", rowActions: [], map: { enabled: false },
};
const ENUMS = { status: [{ value: "working", title: "In service", meaning: "https://vocab.example/station#working" }, { value: "closed", title: "Closed" }] };
createRoot(document.getElementById("root")).render(h("main", { style: { padding: "16px" } }, h(EntityGrid, { config: CONFIG, source, enums: ENUMS, rules: { bikes: { kind: "integer" } }, termHref: (m) => m, locale: "en" })));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

const files: Record<string, string> = {};

test.beforeAll(async () => {
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".griddrill-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({ root: work, logLevel: "silent", build: { outDir: out, emptyOutDir: true, target: "es2022" } });
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(join(dir, entry.name), `${prefix}${entry.name}/`);
      else files[`/${prefix}${entry.name}`] = readFileSync(join(dir, entry.name), "utf8");
    }
  };
  walk(out, "");
  files["/"] = files["/index.html"];
});

for (const width of [1440, 400]) {
  test(`relationship targets open in the same grid, partial data reads by its rule, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 860 });
    const problems: string[] = [];
    page.on("pageerror", (error) => problems.push(error.message));
    await page.route(`${PAGE}**`, (route) => {
      const path = new URL(route.request().url()).pathname;
      const body = files[path];
      if (body === undefined) return route.fulfill({ status: 404, body: "" });
      return route.fulfill({ contentType: path.endsWith(".css") ? "text/css" : path === "/" ? "text/html" : "text/javascript", body });
    });
    await page.goto(PAGE);
    const open = page.getByRole("button", { name: "Open: Company · Helsinki City Transport" }).first();
    await expect(open).toBeVisible();
    // Unreadable target: its id's end, plain, no link and none of its values.
    const hidden = page.getByTitle("urn:ngsi-ld:Company:closed");
    await expect(hidden).toHaveText("closed");
    expect(await hidden.evaluate((el) => el.tagName)).toBe("SPAN");
    await expect(page.getByText("Hidden Owner Ltd")).toHaveCount(0);
    await expect(page.getByRole("link", { name: "In service" }).first()).toHaveAttribute("href", "https://vocab.example/station#working");
    await expect(page.getByText("zzz (not in the list)")).toBeVisible();
    await expect(page.getByText("+2")).toBeVisible();
    const wrong = page.locator(".jc-cell--invalid", { hasText: "n/a" });
    await expect(wrong).toHaveText("n/a (The model expects: Number)");
    // Drawn, not only in a hover title or a screen reader's text: the reason takes room of its own.
    expect((await wrong.boundingBox())?.width ?? 0).toBeGreaterThan(120);
    await expect(wrong).toHaveCSS("border-bottom-style", "dotted");
    await page.screenshot({ path: `${SHOTS}/drill-rows-${width}.png`, fullPage: true });
    if (width === 400) {
      // The cells right of the frozen id, as a person scrolls the table to them.
      for (const [at, name] of [[220, "middle"], [10_000, "end"]] as const) {
        await page.locator(".jc-grid-scroll").first().evaluate((el, left) => { el.scrollLeft = left; }, at);
        await page.screenshot({ path: `${SHOTS}/drill-rows-scrolled-${name}-${width}.png`, fullPage: true });
      }
      await page.locator(".jc-grid-scroll").first().evaluate((el) => { el.scrollLeft = 0; });
    }
    await open.click();
    const trail = page.getByRole("navigation", { name: "Opened links" });
    await expect(trail).toBeVisible();
    await expect(page.getByText("1,400")).toBeVisible();
    await expect(page.getByText("Kamppi", { exact: true })).toBeHidden();
    await page.screenshot({ path: `${SHOTS}/drill-target-${width}.png`, fullPage: true });
    await trail.getByRole("button", { name: "← Back" }).click();
    await expect(trail).toHaveCount(0);
    await expect(page.getByText("Kamppi", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Open: Kamppi" }).click();
    const detail = page.getByRole("dialog");
    await expect(detail.getByTitle("urn:ngsi-ld:Dataset:camera")).toBeVisible();
    await page.screenshot({ path: `${SHOTS}/drill-detail-instances-${width}.png`, fullPage: true });
    expect(problems).toEqual([]);
  });
}
