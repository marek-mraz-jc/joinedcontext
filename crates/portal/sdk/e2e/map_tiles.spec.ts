import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { build } from "vite";

/**
 * A grid's map past the tile threshold, in a real browser (T-5195, T-5189).
 *
 * The page is the grid over 1 500 stations whose source names its Endpoint's tiles; the routes answer
 * the tileset and each tile with real Mapbox Vector Tile bytes, as Antares tiles.rs writes them. The
 * tiles reach `page.route` because the kit reads them through its main-thread protocol: a fetch from
 * MapLibre's own worker would not be intercepted (T-5189).
 *
 * Checked: the tileset is read once, every tile is read on the page's own host under the Endpoint,
 * without a query and as MVT; the count line says the tiles draw every entity; a click on a shape
 * only a tile holds opens that entity's detail, read by its URN. Screenshots at 1440 and 400 go to
 * JC_EVIDENCE_DIR when it is set.
 */
const PAGE = "http://maptiles.test/";
const TILESET = "/api/endpoint/air/ogc/features/collections/Station/tiles/WebMercatorQuad";
const COLS = 50;

const station = (k: number) => ({
  id: `urn:ngsi-ld:Station:${k}`,
  name: `Station ${k}`,
  lon: 17.1 + (k % COLS) * 0.002,
  lat: 48.14 + Math.floor(k / COLS) * 0.002,
});
const STATIONS = Array.from({ length: 1500 }, (_, k) => station(k));

const ENTRY = `
import { createRoot } from "react-dom/client";
import { createElement as h } from "react";
import { EntityGrid } from "../../src/grid/EntityGrid";
import { fixtureSource } from "../../src/grid/source";

const STATIONS = ${JSON.stringify(STATIONS)}.map((s) => ({
  id: s.id, type: "Station",
  name: { type: "Property", value: s.name },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [s.lon, s.lat] } },
}));

const CONFIG = {
  source: { kind: "fixture", name: "stations" },
  type: "Station",
  columns: [{ attr: "name" }],
  entityTimestamps: false,
  filters: {},
  pageSize: 10,
  mode: "view",
  editableAttrs: [],
  history: { enabled: false },
  density: "comfortable",
  rowActions: [],
  map: { enabled: true, attr: "location", position: "right" },
};

// The Endpoint source's capability, on the fixture's rows: its tiles live under this Endpoint.
const source = { ...fixtureSource(STATIONS), tilesAt: "/api/endpoint/air" };
createRoot(document.getElementById("root")).render(h(EntityGrid, { config: CONFIG, source }));
`;

const HTML = `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>.jc-grid-map .maplibregl-map { width: 100%; height: 420px; }</style>
</head><body><div id="root"></div><script type="module" src="./entry.jsx"></script></body></html>`;

/** A Mapbox Vector Tile 2.1 of the stations in tile z/x/y: one layer, each URN under `id`. */
function mvt(layer: string, z: number, x: number, y: number): Buffer | null {
  const varint = (out: number[], v: number) => {
    while (v > 127) {
      out.push((v & 127) | 128);
      v = Math.floor(v / 128);
    }
    out.push(v);
  };
  const bytes = (out: number[], field: number, b: Iterable<number>) => {
    const list = [...b];
    varint(out, field * 8 + 2);
    varint(out, list.length);
    out.push(...list);
  };
  const zz = (n: number) => (n << 1) ^ (n >> 31);
  const n = 2 ** z;
  const values: string[] = [];
  const body: number[] = [];
  varint(body, 15 * 8);
  varint(body, 2);
  bytes(body, 1, Buffer.from(layer));
  let count = 0;
  for (const s of STATIONS) {
    const r = (s.lat * Math.PI) / 180;
    const px = Math.round((((s.lon + 180) / 360) * n - x) * 4096);
    const py = Math.round(((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2 * n - y) * 4096);
    if (px < 0 || px >= 4096 || py < 0 || py >= 4096) continue;
    count += 1;
    const feature: number[] = [];
    const packed = (field: number, list: number[]) => {
      const p: number[] = [];
      list.forEach((v) => varint(p, v));
      bytes(feature, field, p);
    };
    packed(2, [0, values.length]);
    values.push(s.id);
    varint(feature, 3 * 8);
    varint(feature, 1);
    packed(4, [9, zz(px), zz(py)]);
    bytes(body, 2, feature);
  }
  bytes(body, 3, Buffer.from("id"));
  for (const v of values) {
    const value: number[] = [];
    bytes(value, 1, Buffer.from(v));
    bytes(body, 4, value);
  }
  varint(body, 5 * 8);
  varint(body, 4096);
  const tile: number[] = [];
  bytes(tile, 3, body);
  return count ? Buffer.from(tile) : null;
}

let files: Record<string, string> = {};

test.beforeAll(async () => {
  const work = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", ".maptiles-fixture");
  mkdirSync(work, { recursive: true });
  writeFileSync(join(work, "entry.jsx"), ENTRY);
  writeFileSync(join(work, "index.html"), HTML);
  const out = join(work, "dist");
  await build({ root: work, logLevel: "silent", build: { outDir: out, emptyOutDir: true, target: "es2022" } });
  files = {};
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(join(dir, entry.name), `${prefix}${entry.name}/`);
      else files[`/${prefix}${entry.name}`] = readFileSync(join(dir, entry.name), "utf8");
    }
  };
  walk(out, "");
  files["/"] = files["/index.html"];
  // The worker and the module it imports, from the package: vite copies neither into the bundle.
  for (const name of ["maplibre-gl-worker.mjs", "maplibre-gl-shared.mjs"]) {
    files[`/assets/${name}`] = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules", "maplibre-gl", "dist", name), "utf8");
  }
});

for (const width of [1440, 400]) {
  test(`past the threshold the map draws every station from the Endpoint's tiles (${width} px)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const problems: string[] = [];
    const tilesets: string[] = [];
    const tiles: { url: string; accept: string }[] = [];
    page.on("pageerror", (error) => problems.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") problems.push(message.text());
    });
    await page.route("**/*", (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== new URL(PAGE).origin) return route.fulfill({ status: 599, body: "" });
      if (url.pathname === TILESET) {
        tilesets.push(request.url());
        const item = `https://api.example.com${TILESET}/{tileMatrix}/{tileRow}/{tileCol}`;
        return route.fulfill({ contentType: "application/json", body: JSON.stringify({ links: [{ rel: "item", type: "application/vnd.mapbox-vector-tile", templated: true, href: item }] }) });
      }
      if (url.pathname.startsWith(`${TILESET}/`)) {
        tiles.push({ url: request.url(), accept: request.headers()["accept"] ?? "" });
        const [z, y, x] = url.pathname.slice(TILESET.length + 1).split("/").map(Number);
        const tile = mvt("Station", z, x, y);
        return tile ? route.fulfill({ contentType: "application/vnd.mapbox-vector-tile", body: tile }) : route.fulfill({ status: 204, body: "" });
      }
      const body = files[url.pathname];
      if (body === undefined) return route.fulfill({ status: 404, body: "" });
      return route.fulfill({ contentType: url.pathname.endsWith(".css") ? "text/css" : url.pathname === "/" ? "text/html" : "text/javascript", body });
    });
    await page.goto(PAGE);

    await expect(page.getByText("Every entity, from the Endpoint's vector tiles")).toBeVisible();
    await expect.poll(() => tiles.length).toBeGreaterThan(0);
    await page.waitForFunction(() => {
      const map = (window as unknown as { kitGeoMap?: { loaded(): boolean; areTilesLoaded(): boolean } }).kitGeoMap;
      return Boolean(map?.loaded() && map.areTilesLoaded());
    });
    expect(tilesets).toEqual([`${PAGE.slice(0, -1)}${TILESET}`]);
    for (const tile of tiles) {
      const url = new URL(tile.url);
      expect(url.origin).toBe(new URL(PAGE).origin);
      expect(url.pathname).toMatch(new RegExp(`^${TILESET}/\\d+/\\d+/\\d+$`));
      expect(url.search).toBe("");
      expect(tile.accept).toBe("application/vnd.mapbox-vector-tile");
    }
    const dir = process.env.JC_EVIDENCE_DIR;
    if (dir) await page.screenshot({ path: join(dir, `sdk-grid-map-tiled-${width}.png`), fullPage: true });

    // Station 55 is on no loaded page (the page holds 0 to 9): only a tile draws it.
    const target = station(55);
    // Always at street zoom: a low zoom keeps the station inside the view, but its 0.002 degree
    // neighbours overlap it there and the click lands on one of them.
    await page.evaluate(([lon, lat]) => (window as unknown as { kitGeoMap: { jumpTo(o: unknown): void } }).kitGeoMap.jumpTo({ center: [lon, lat], zoom: 15 }), [target.lon, target.lat]);
    await page.waitForFunction(() => (window as unknown as { kitGeoMap: { areTilesLoaded(): boolean } }).kitGeoMap.areTilesLoaded());
    const point = await page.evaluate(([lon, lat]) => {
      const map = (window as unknown as { kitGeoMap: { project(p: [number, number]): { x: number; y: number }; getCanvas(): HTMLCanvasElement } }).kitGeoMap;
      const box = map.getCanvas().getBoundingClientRect();
      const p = map.project([lon, lat]);
      return { x: box.left + p.x, y: box.top + p.y };
    }, [target.lon, target.lat] as [number, number]);
    await page.mouse.click(point.x, point.y);
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Station 55");
    if (dir) await page.screenshot({ path: join(dir, `sdk-grid-map-tiled-detail-${width}.png`), fullPage: true });
    expect(problems).toEqual([]);
  });
}
