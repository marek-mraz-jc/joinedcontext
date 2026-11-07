import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * The geometry editor in a real browser (T-1442, UI-72, SDK-29).
 *
 * The unit tests mock the map, so nothing there proves that Terra Draw is wired to MapLibre at all:
 * that a click on the canvas becomes a corner, that a vertex can be dragged, that what comes out is
 * RFC 7946. This page is the editor and nothing else, built from the sources with vite, served from
 * memory. There is no basemap — the kit's own fallback background is used — so no tile leaves the
 * browser and the check is offline.
 */
const PAGE = "http://geo.test/";

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h, useState } from "react";
import { GeoEditor } from "../../src/geo/GeoEditor";

// One editor, the type chosen by the address: a station is a Point, a district is a Polygon, and
// the "done when" of T-1442 is both of them out of the same component.
function Harness() {
  const [value, setValue] = useState(null);
  const allowed = location.hash === "#point" ? ["Point"] : ["Polygon"];
  return h("div", null,
    h(GeoEditor, { value, onChange: setValue, allowed }),
    h("pre", { "data-testid": "saved" }, value ? JSON.stringify(value) : ""),
  );
}
createRoot(document.getElementById("root")).render(h(Harness));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8">
<style>.map { width: 800px; height: 500px; }</style>
</head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

let files: Record<string, string> = {};

test.beforeAll(async () => {
  // Inside the package, not in /tmp: the entry imports `../src` and needs this `node_modules`.
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".geo-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({
    root: work,
    logLevel: "silent",
    build: { outDir: out, emptyOutDir: true, target: "es2022" },
  });
  // Every file the build wrote, not only the ones index.html names: the drawing library is a
  // dynamic import, so it is a chunk the page fetches at runtime and a 404 there is silent.
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
  // The library loads its own worker beside its script and vite does not copy it into the bundle,
  // so it is served from the package it belongs to. Without it no style ever finishes loading.
  files["/assets/maplibre-gl-worker.mjs"] = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", "maplibre-gl", "dist", "maplibre-gl-worker.mjs"),
    "utf8",
  );
});

/** The built page, served from memory: no server, no network, no basemap. */
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

/** The map is up and the drawing library is wired when the style holds more than its background. */
async function drawable(page: import("@playwright/test").Page): Promise<void> {
  await page.waitForFunction(() => {
    const m = (window as never as { kitGeoMap?: { getStyle(): { layers: unknown[] } } }).kitGeoMap;
    const layers = m?.getStyle?.()?.layers?.length ?? 0;
    // The kit's fallback style has one background layer; the rest are Terra Draw's own, so more
    // than one means the style is up and the drawing library is wired to it.
    return layers > 1;
  });
}

test("a polygon is drawn, a vertex is dragged, a vertex is removed, and what is saved is RFC 7946", async ({
  page,
}) => {
  const problems: string[] = [];
  await serve(page, problems);
  await page.goto(`${PAGE}#polygon`);

  const map = page.getByTestId("geo-editor-map");
  await expect(map).toBeVisible();
  // The map is up when its style has loaded; `loaded()` also waits for tiles, and this style has
  // none (no basemap is configured, so the kit draws its own background).
  await page.waitForFunction(() => {
    const m = (window as never as { kitGeoMap?: { getStyle(): { layers: unknown[] } } }).kitGeoMap;
    const layers = m?.getStyle?.()?.layers?.length ?? 0;
    // The kit's fallback style has one background layer; the rest are Terra Draw's own, so more
    // than one means the style is up and the drawing library is wired to it.
    return layers > 1;
  });

  await page.getByRole("button", { name: "Area" }).click();
  const box = (await map.boundingBox())!;
  const corners = [
    { x: box.x + 200, y: box.y + 150 },
    { x: box.x + 500, y: box.y + 150 },
    { x: box.x + 500, y: box.y + 350 },
    { x: box.x + 200, y: box.y + 350 },
  ];
  for (const corner of corners) {
    await page.mouse.click(corner.x, corner.y);
  }
  // Terra Draw closes a polygon on a second click at the last corner.
  await page.mouse.click(corners[3].x, corners[3].y);

  const saved = page.getByTestId("saved");
  await expect(saved).toContainText('"type":"Polygon"');
  const first = JSON.parse(await saved.innerText()) as { type: string; coordinates: number[][][] };
  // Four corners and the position that closes the ring back on the first one.
  expect(first.coordinates[0]).toHaveLength(5);
  expect(first.coordinates[0][4]).toEqual(first.coordinates[0][0]);
  for (const [longitude, latitude] of first.coordinates[0]) {
    expect(Math.abs(longitude)).toBeLessThanOrEqual(180);
    expect(Math.abs(latitude)).toBeLessThanOrEqual(90);
  }
  // Five positions in the table, each one a pair of inputs a keyboard can reach.
  await expect(page.getByRole("row")).toHaveCount(6); // the header and five points

  // Drag on the shape in select mode. Terra Draw drags a vertex, and on a midpoint it inserts one
  // and drags that, so what is asserted is what both mean: the geometry changed, through the
  // library, and what came out is still a closed ring inside the world's bounds.
  await page.getByRole("button", { name: "Select" }).click();
  // A shape is selected before its vertices can be dragged: that is what select mode means.
  await page.mouse.click(box.x + 350, box.y + 250);
  const grabbed = await page.evaluate(
    (position) =>
      (window as never as { kitGeoMap: { project(p: number[]): { x: number; y: number } } }).kitGeoMap.project(position),
    first.coordinates[0][1],
  );
  // `project` answers in the canvas's own coordinates; the mouse moves in the viewport's.
  await page.mouse.move(box.x + grabbed.x, box.y + grabbed.y);
  await page.mouse.down();
  await page.mouse.move(box.x + grabbed.x + 90, box.y + grabbed.y - 50, { steps: 12 });
  await page.mouse.up();
  await expect.poll(async () => await saved.innerText()).not.toBe(JSON.stringify(first));
  const dragged = JSON.parse(await saved.innerText()) as typeof first;
  expect(dragged.coordinates[0][dragged.coordinates[0].length - 1]).toEqual(dragged.coordinates[0][0]);
  for (const [longitude, latitude] of dragged.coordinates[0]) {
    expect(Math.abs(longitude)).toBeLessThanOrEqual(180);
    expect(Math.abs(latitude)).toBeLessThanOrEqual(90);
  }

  // And the keyboard's way: remove a point from the table, which is the same edit without a mouse.
  await page.getByRole("button", { name: "Remove point 1.3" }).click();
  const trimmed = JSON.parse(await saved.innerText()) as typeof first;
  expect(trimmed.coordinates[0]).toHaveLength(dragged.coordinates[0].length - 1);
  expect(trimmed.coordinates[0][trimmed.coordinates[0].length - 1]).toEqual(trimmed.coordinates[0][0]);

  expect(problems).toEqual([]);
});

test("a station's point is drawn and corrected by keyboard, and what is saved is RFC 7946", async ({ page }) => {
  const problems: string[] = [];
  await serve(page, problems);
  await page.goto(`${PAGE}#point`);
  const map = page.getByTestId("geo-editor-map");
  await expect(map).toBeVisible();
  await drawable(page);

  // Only the one mode a point field allows.
  await expect(page.getByRole("button", { name: "Point" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Area" })).toHaveCount(0);

  await page.getByRole("button", { name: "Point" }).click();
  const box = (await map.boundingBox())!;
  await page.mouse.click(box.x + 300, box.y + 250);

  const saved = page.getByTestId("saved");
  await expect(saved).toContainText('"type":"Point"');
  const drawn = JSON.parse(await saved.innerText()) as { type: string; coordinates: number[] };
  expect(drawn.coordinates).toHaveLength(2);
  expect(Math.abs(drawn.coordinates[0])).toBeLessThanOrEqual(180);
  expect(Math.abs(drawn.coordinates[1])).toBeLessThanOrEqual(90);

  // The exact correction a person types when the click was a pixel off — no mouse involved.
  const longitude = page.getByLabel("Point longitude");
  await longitude.fill("24.9384");
  await longitude.blur();
  const corrected = JSON.parse(await saved.innerText()) as typeof drawn;
  expect(corrected.coordinates[0]).toBe(24.9384);
  expect(corrected.coordinates[1]).toBe(drawn.coordinates[1]);

  // And the refusal a person sees instead of a silent write.
  const latitude = page.getByLabel("Point latitude");
  await latitude.fill("600");
  await latitude.blur();
  await expect(page.getByRole("alert")).toContainText("latitude 600 is outside ±90");
  expect((JSON.parse(await saved.innerText()) as typeof drawn).coordinates[1]).toBe(drawn.coordinates[1]);

  expect(problems).toEqual([]);
});
