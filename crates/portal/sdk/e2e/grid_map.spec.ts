import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * The grid and its map in a real browser (T-1443, UI-72, UI-67).
 *
 * The unit tests mock MapLibre, so nothing there proves the panel is mounted on a real canvas at all.
 * This page is the grid with `map.enabled`, built from the sources with vite and served from memory —
 * no basemap, so no tile leaves the browser and the check is offline.
 *
 * The drawn area is checked here with a real trusted click, because that is what found T-2276: the
 * answer re-derived the question, so one click started a query that never stopped and the page
 * stopped answering. A programmatic click passes either way and proves nothing.
 *
 * What this leg does NOT cover, and T-1443 leaves open: clicking a shape to activate its row and
 * dragging a point in the panel. `GeoView` publishes no map handle (only `GeoEditor` does), so a
 * shape cannot be located from the test.
 */
const PAGE = "http://gridmap.test/";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h, useState } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource, SourceError } from "../../src/grid/source";
import { applyChanges } from "../../src/grid/apply";

const STATIONS = [
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:001", type: "Station",
    name: { type: "Property", value: "Kamppi" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } } },
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:002", type: "Station",
    name: { type: "Property", value: "Kallio" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.951, 60.186] } } },
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:003", type: "Station",
    name: { type: "Property", value: "No geometry yet" } },
];

const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "Station",
  columns: [{ attr: "name" }, { attr: "location" }],
  entityTimestamps: false,
  filters: {},
  pageSize: 50,
  mode: "edit",
  editableAttrs: ["name", "location"],
  history: { enabled: false },
  density: "comfortable",
  rowActions: [],
  map: { enabled: true, attr: "location", position: "right" },
};

// The source the grid reads, and the PATCH bodies it would send, kept for the assertion: this is
// what travels to the Endpoint, which is the only place a geometry's own type can be wrong.
const sent = [];
const base = fixtureSource(STATIONS);
// ?refuse=401 or 403: the private-dataset states (T-6376), with no row ever read.
const refuse = Number(new URLSearchParams(location.search).get("refuse") ?? 0);
const source = refuse
  ? { ...base, query: async () => { throw new SourceError(refuse, refuse === 403 ? "no policy grants you Station on this endpoint" : "sign in"); } }
  : { ...base, patch: async (id, body) => { sent.push({ id, body }); } };

function Harness() {
  const [applied, setApplied] = useState(null);
  return h("div", null,
    h(EntityGrid, {
      config: CONFIG,
      source,
      onSignIn: () => { window.kitSignIn = true; },
    }),
    h("button", {
      "data-testid": "apply-through-source",
      onClick: async () => {
        // The grid's own review dialog is covered by the unit tests; what this page proves is that
        // the pending geometry reaches the source as a GeoProperty.
        const pending = window.kitPending ?? [];
        await applyChanges({ reason: "Sensor stuck", source, entities: pending, observed: "keep" });
        setApplied(JSON.stringify(sent));
      },
    }, "apply through the source"),
    h("pre", { "data-testid": "sent" }, applied ?? ""),
  );
}
createRoot(document.getElementById("root")).render(h(Harness));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8">
</head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

let files: Record<string, string> = {};

test.beforeAll(async () => {
  // Inside the package, not in /tmp: the entry imports `../src` and needs this `node_modules`.
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".gridmap-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({ root: work, logLevel: "silent", build: { outDir: out, emptyOutDir: true, target: "es2022" } });
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
  // Served from the package it belongs to: vite does not copy the worker into the bundle, and
  // without it no style ever finishes loading.
  // maplibre 6's worker imports `maplibre-gl-shared.mjs`: without it the map never loads (T-5195).
  for (const name of ["maplibre-gl-worker.mjs", "maplibre-gl-shared.mjs"]) {
    files[`/assets/${name}`] = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", "maplibre-gl", "dist", name), "utf8");
  }
});

async function shot(page: import("@playwright/test").Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}-${page.viewportSize()?.width ?? 0}.png`), fullPage: true });
}

async function serve(page: import("@playwright/test").Page, problems: string[]): Promise<void> {
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

for (const width of [1440, 400])
test(`the panel mounts a real map beside the rows, with the page's shapes on it, at ${width} px`, async ({
  page,
}) => {
  await page.setViewportSize({ width, height: 900 });
  const problems: string[] = [];
  await serve(page, problems);
  await page.goto(PAGE);

  // The rows are there, and so is the panel, with the one station that has no geometry counted off
  // the map rather than hidden.
  await expect(page.getByRole("textbox", { name: /name/ }).first()).toHaveValue("Kamppi");
  await expect(page.locator("section.jc-grid-map")).toBeVisible();
  await expect(page.getByText("not on the map: 1")).toBeVisible();

  // A real MapLibre canvas, with the two stations that have a geometry on it.
  await expect(page.locator(".jc-grid-map .maplibregl-map")).toBeVisible();
  await expect(page.getByText("2 shapes")).toBeVisible();
  await page.waitForFunction(() => (window as unknown as { kitGeoMap?: { loaded(): boolean } }).kitGeoMap?.loaded() === true);

  await shot(page, "map-closed");

  // The drawn area, by real clicks on the canvas (T-2276, T-6345): Draw area opens the tools, a
  // rectangle is two clicks, and the rows narrow to it; the page still answers afterwards.
  await page.getByRole("button", { name: "Draw area" }).click();
  // The style is up and Terra Draw wired when it holds more than the kit's background layer.
  await page.waitForFunction(() => ((window as never as { kitGeoMap?: { getStyle(): { layers: unknown[] } } }).kitGeoMap?.getStyle?.()?.layers?.length ?? 0) > 1);
  await page.getByRole("button", { name: "Area", exact: true }).click();
  await expect(page.getByText(/click the map/)).toBeVisible();
  await shot(page, "drawing");
  const map = (await page.getByTestId("geo-editor-map").boundingBox())!;
  // Draw inside what the viewport shows, however wide the panel is.
  const box = { ...map, width: Math.min(map.width, (page.viewportSize()?.width ?? 0) - map.x - 8) };
  const corners = [
    [0.15, 0.15],
    [0.85, 0.15],
    [0.85, 0.85],
    [0.15, 0.85],
  ].map(([x, y]) => ({ x: box.x + box.width * x, y: box.y + box.height * y }));
  for (const corner of corners) await page.mouse.click(corner.x, corner.y);
  // Terra Draw closes a polygon on a second click at the last corner.
  await page.mouse.click(corners[3].x, corners[3].y);
  await expect(page.getByText("Filtered to the drawn area")).toBeVisible();
  await expect(page.getByRole("button", { name: "Clear area" }).first()).toBeVisible();
  await shot(page, "area-set");
  await page.getByRole("button", { name: "Clear area" }).first().click();
  await expect(page.getByText("Filtered to the drawn area")).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /name/ }).first()).toHaveValue("Kamppi");

  expect(problems).toEqual([]);
});

for (const width of [1440, 400]) {
  test(`a private dataset says sign in or no access, never an empty grid, at ${width} px (T-6376)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const problems: string[] = [];
    await serve(page, problems);
    await page.goto(`${PAGE}?refuse=401`);
    await expect(page.getByRole("heading", { name: "Sign in to see this dataset" })).toBeVisible();
    await expect(page.getByRole("grid")).toHaveCount(0);
    await shot(page, "signed-out");
    await page.getByRole("button", { name: "Sign in" }).click();
    expect(await page.evaluate(() => (window as unknown as { kitSignIn?: boolean }).kitSignIn)).toBe(true);
    await page.goto(`${PAGE}?refuse=403`);
    await expect(page.getByRole("heading", { name: "You have no access to this dataset" })).toBeVisible();
    await expect(page.getByText("no policy grants you Station on this endpoint")).toBeVisible();
    await shot(page, "no-access");
    expect(problems).toEqual([]);
  });
}
