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
const PAGE = "http://gridreview.test/";
const SHOTS = process.env.SHOTS_DIR ?? "test-results/grid-review";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource } from "../../src/grid/source";

const ID = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:mobility:001";
const STATIONS = [
  { id: ID, type: "BikeHireDockingStation",
    name: { type: "Property", value: "Kamppi" },
    status: { type: "Property", value: "working" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } },
    refPointOfInterest: { type: "Relationship", object: "urn:ngsi-ld:PointOfInterest:hel.fi:poi:kamppi-centre" } },
];
const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "BikeHireDockingStation",
  columns: [{ attr: "name", label: "Name" }, { attr: "status", label: "Status" }, { attr: "location", label: "Location" }, { attr: "refPointOfInterest", label: "Point of interest" }],
  entityTimestamps: false,
  filters: {},
  pageSize: 50,
  mode: "edit",
  editableAttrs: ["name", "location", "refPointOfInterest"],
  history: { enabled: false },
  density: "comfortable",
  rowActions: [],
  map: { enabled: false },
};
const row = (name, write, reason) => ({ name, read: true, write, delete: false, reason });
const ACCESS = [row("name", true, "grant"), row("status", false, "prohibited"), row("location", true, "grant"), row("refPointOfInterest", true, "grant")];
const source = { ...fixtureSource(STATIONS), patch: async () => {} };
const EDITS = { [ID]: {
  location: { type: "Point", coordinates: [24.9525, 60.1841] },
  refPointOfInterest: "urn:ngsi-ld:PointOfInterest:hel.fi:poi:kallio-library",
} };
createRoot(document.getElementById("root")).render(h(EntityGrid, { config: CONFIG, source, access: ACCESS, state: { edits: EDITS } }));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

const files: Record<string, string> = {};

test.beforeAll(async () => {
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".gridreview-fixture");
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
  test(`the review of a GeoProperty and a Relationship change reads as words at ${width} px`, async ({ page }) => {
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
    await expect(page.getByRole("textbox", { name: /name/i }).first()).toHaveValue("Kamppi");
    await expect(page.getByRole("img", { name: "Read-only: Explicitly not allowed by a policy." })).toBeAttached();
    await page.getByRole("button", { name: "Review the changes" }).click();
    const review = page.getByRole("region", { name: "Review the changes" }).first();
    await expect(review.getByRole("row", { name: /Location/ })).toContainText("Kamppi");
    await expect(review.getByRole("row", { name: /Point of interest/ })).toContainText("kallio-library");
    const text = (await review.textContent()) ?? "";
    expect(text).not.toContain("urn:");
    expect(text).not.toContain("[object Object]");
    await expect(review.getByRole("button", { name: "Copy the identifier: Kamppi" }).first()).toBeVisible();
    await review.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${SHOTS}/review-geo-relationship-${width}.png`, fullPage: true });
    expect(problems).toEqual([]);
  });
}
