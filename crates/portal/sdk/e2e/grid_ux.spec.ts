import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * The shared grid as a person meets it in a real browser (T-7063): on a 400 px phone the page never
 * scrolls sideways while the table does, the header stays in sight as the rows scroll under it, and
 * a keyboard alone opens a row and comes back to the grid. Built from the sources, served from memory.
 */
const PAGE = "http://gridux.test/";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource } from "../../src/grid/source";

const p = (value) => ({ type: "Property", value });
const STATIONS = Array.from({ length: 60 }, (_, i) => ({
  id: "urn:ngsi-ld:Station:hel:" + String(i + 1).padStart(3, "0"),
  type: "Station",
  name: p("Station " + (i + 1)),
  bikes: p(i % 13),
  district: p("Central business district of Helsinki, block " + (i % 7)),
  seen: p("2026-11-1" + (i % 9) + "T08:30:00Z"),
}));
const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "Station",
  columns: [{ attr: "name", label: "Name" }, { attr: "bikes", label: "Bikes" }, { attr: "district", label: "District" }, { attr: "seen", label: "Seen" }],
  entityTimestamps: false, filters: {}, pageSize: 60, mode: "view", editableAttrs: [], history: { enabled: false },
  density: "comfortable", rowActions: [], map: { enabled: false },
};
createRoot(document.getElementById("root")).render(h("main", { style: { padding: "8px" } }, h(EntityGrid, { config: CONFIG, source: fixtureSource(STATIONS), locale: "en" })));
`;

const HTML = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Grid</title></head><body style="margin:0"><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

const files: Record<string, string> = {};

test.beforeAll(async () => {
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".gridux-fixture");
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

async function open(page: import("@playwright/test").Page, width: number, problems: string[]): Promise<void> {
  await page.setViewportSize({ width, height: 800 });
  page.on("pageerror", (error) => problems.push(error.message));
  await page.route(`${PAGE}**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    const body = files[path];
    if (body === undefined) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ contentType: path.endsWith(".css") ? "text/css" : path === "/" ? "text/html" : "text/javascript", body });
  });
  await page.goto(PAGE);
  await expect(page.getByRole("button", { name: "Open: Station 1", exact: true })).toBeVisible();
}

test("on a 400 px phone the page stays put, the table scrolls sideways and its header stays in sight", async ({ page }) => {
  const problems: string[] = [];
  await open(page, 400, problems);
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
  const box = page.locator(".jc-grid-scroll").first();
  const sideways = await box.evaluate((el) => {
    el.scrollLeft = 200;
    return el.scrollLeft;
  });
  expect(sideways).toBeGreaterThan(0);
  const gap = await box.evaluate((el) => {
    el.scrollLeft = 0;
    el.scrollTop = 800;
    const head = el.querySelector("thead")!.getBoundingClientRect().top;
    return Math.abs(head - el.getBoundingClientRect().top);
  });
  expect(gap).toBeLessThanOrEqual(1);
  expect(problems).toEqual([]);
});

test("a keyboard alone opens a row and comes back to the grid", async ({ page }) => {
  const problems: string[] = [];
  await open(page, 1440, problems);
  const grid = page.getByRole("grid");
  await grid.focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: "Open: Station 1", exact: true })).toBeFocused();
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", { name: "Details: Station 1", exact: true });
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(grid).toBeFocused();
  expect(problems).toEqual([]);
});
