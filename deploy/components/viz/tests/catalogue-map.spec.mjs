// T-5061: the catalogue's dataset map (jc_theme jc-map.js, T-4639) in a browser. It reads the
// Endpoint's OGC API Features items live, page by page and, past 5 000 features, by the view's bbox;
// it never reads a copied file or a DataStore table and never sends a credential. Every host is
// routed inside the test: the catalogue page is the embed page (templates__jc__map.html) with its
// config as jc_map() builds it, the Endpoint answers like the gateway's OGC items. Each state leaves
// a screenshot in test-results/catalogue-map/ (or $T5061_SHOTS) for the UX read in the task.
//   npm i --no-save --no-package-lock @playwright/test@1.63.0 (chromium 1243, as the Portal pins)
//   PLAYWRIGHT_BROWSERS_PATH=/ms-playwright npx playwright test -c components/viz/tests catalogue-map
import { test, expect } from "@playwright/test";
import { readFileSync, mkdirSync } from "node:fs";
import { gunzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { mvt } from "./mvt.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const theme = path.join(here, "../../../charts/ckan/files/jc_theme");
const vendor = path.join(here, "../../../images/viz/vendor");
const shots = process.env.T5061_SHOTS || path.join(here, "test-results/catalogue-map");
mkdirSync(shots, { recursive: true });
const DOMAIN = "dev.joinedcontext.com";
const CATALOGUE = `https://opendata.${DOMAIN}`;
const SLUG = "4ysiqy53pz6p6x2c5xkqz3w4b6m5n2rt";
const OGC = `https://api.${DOMAIN}/api/endpoint/${SLUG}/ogc/features/collections`;
const TEXT = {
  map_cap: "Showing the first {n}; zoom in to see the rest.",
  map_failed: "The map could not load its data; the table above still works.",
  map_empty: "This dataset has no places to show on a map.",
  map_loading: "Loading the map's data…", map_close: "Close", map_panel: "Selected place", map_length: "Length",
  map_area: "Area", map_keys: "Arrow keys move the map, + and - zoom, Enter opens the place in the middle.",
  map_title: "Map of the data", map_more: "More"
};

const MANY = 12000;
let TILED = null; // built at first use: dock() is defined below
const MVT = "application/vnd.mapbox-vector-tile";

const dock = (i) => ({ type: "Feature", id: `urn:ngsi-ld:BikeHireDockingStation:hel:${i}`,
  geometry: { type: "Point", coordinates: [24.7 + (i % 120) * 0.004, 60.1 + Math.floor(i / 120) * 0.002] },
  properties: { name: `Asema ${i}`, availableBikeNumber: i % 20, status: "working" } });
const area = (i) => {
  const [x, y] = [19.1 + (i % 5) * 0.03, 48.7 + Math.floor(i / 5) * 0.02];
  return { type: "Feature", id: `urn:ngsi-ld:AdministrativeArea:bb:${i}`,
    geometry: { type: "Polygon", coordinates: [[[x, y], [x + 0.025, y], [x + 0.025, y + 0.015], [x, y + 0.015], [x, y]]] },
    properties: { name: `Mestská časť ${i + 1}`, population: 12000 + i } };
};
// What each data set holds, before paging.
const DATA = {
  points: { type: "BikeHireDockingStation", all: () => Array.from({ length: 40 }, (_, i) => dock(i)) },
  polygons: { type: "AdministrativeArea", all: () => Array.from({ length: 15 }, (_, i) => area(i)) },
  empty: { type: "BikeHireDockingStation", all: () => [] },
  many: { type: "BikeHireDockingStation", all: () => Array.from({ length: MANY }, (_, i) => dock(i)) },
  // T-5189: past the cap with a tileset, the map draws the Endpoint's vector tiles.
  tiled: { type: "BikeHireDockingStation", tiles: true, all: () => (TILED = TILED || Array.from({ length: 60000 }, (_, i) => dock(i))) },
  failing: { type: "BikeHireDockingStation", status: 502 }
};

function page(html, lang) {
  return `<!doctype html><html lang="${lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Map | Catalogue</title><style>:root { --jc-primary: #1d4ed8; --jc-primary-fg: #ffffff; --jc-primary-rgb: 29, 78, 216; --jc-secondary: #0f766e;
--jc-accent: #f59e0b; --jc-background: #ffffff; --jc-text: #0f172a; }</style><link rel="stylesheet" href="/jc-theme.css"><link rel="stylesheet" href="/maplibre-gl-6.14.0/maplibre-gl.css"></head>
<body class="jc-embed"><main class="jc-map-page">${html}</main><script type="module" src="/jc-map.js"></script></body></html>`;
}

function mapHtml(kind) {
  const type = DATA[kind].type;
  const config = {
    layers: [Object.assign({ type, items: `${OGC}/${type}/items`, schema: `${OGC}/${type}/schema` },
      DATA[kind].tiles ? { tiles: `${OGC}/${type}/tiles/WebMercatorQuad` } : {})], basemap: {},
    lib: "/maplibre-gl-6.14.0/maplibre-gl.mjs", pmtiles: "", enums: "/jc-enums.json", lang: "en", title: `Dataset ${kind}`,
    dataset: `${CATALOGUE}/dataset/${kind}`, embed: `${CATALOGUE}/map/${kind}`, t: TEXT
  };
  // As the template writes it: JSON in a single-quoted attribute, the quote escaped.
  const attr = JSON.stringify(config).replaceAll("'", "\\u0027");
  return `<div class="jc-map" data-jc-map='${attr}'><div class="jc-map-area" role="application" aria-label="${TEXT.map_title}: Dataset ${kind}" aria-describedby="jc-map-keys"></div>
<p class="jc-map-note" role="status"></p><p id="jc-map-keys" class="visually-hidden">${TEXT.map_keys}</p></div>`;
}

async function route(browserPage, kind) {
  const asked = [];
  await browserPage.route("**/*", async (r) => {
    const request = r.request();
    const url = new URL(request.url());
    const send = (body, type, status = 200) => r.fulfill({ status, contentType: type, body });
    if (url.origin === CATALOGUE) {
      if (url.pathname.startsWith("/map/")) return send(page(mapHtml(kind), "en"), "text/html");
      if (url.pathname === "/jc-map.js") return send(readFileSync(path.join(theme, "public__jc-map.js")), "text/javascript");
      if (url.pathname === "/jc-theme.css") return send(readFileSync(path.join(theme, "public__jc-theme.css")), "text/css");
      if (url.pathname === "/jc-enums.json") return send(readFileSync(path.join(theme, "public__jc-enums.json")), "application/json");
      if (url.pathname === "/maplibre-gl-6.14.0/maplibre-gl.mjs") return send(gunzipSync(readFileSync(path.join(vendor, "maplibre-gl-6.14.0.mjs.gz"))), "text/javascript");
      if (url.pathname === "/maplibre-gl-6.14.0/maplibre-gl-worker.mjs") return send(gunzipSync(readFileSync(path.join(vendor, "maplibre-gl-worker-6.14.0.mjs.gz"))), "text/javascript");
      if (url.pathname === "/maplibre-gl-6.14.0/maplibre-gl.css") return send(readFileSync(path.join(vendor, "maplibre-gl-6.14.0.css")), "text/css");
      return send("", "text/plain", 404);
    }
    if (url.href.startsWith(OGC)) {
      asked.push({ url, headers: request.headers() });
      if (url.pathname.endsWith("/schema")) return send(JSON.stringify({ properties: {} }), "application/schema+json");
      const data = DATA[kind];
      if (data.status) return send(JSON.stringify({ code: "BadGateway", description: "upstream" }), "application/json", data.status);
      let all = data.all();
      const tile = /\/tiles\/WebMercatorQuad(?:\/(\d+)\/(\d+)\/(\d+))?$/.exec(url.pathname);
      if (tile && !data.tiles) return send("", "text/plain", 404);
      if (tile && !tile[1]) {
        return send(JSON.stringify({ dataType: "vector", links: [{ rel: "item", type: MVT, templated: true,
          href: `${OGC}/${data.type}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}` }] }), "application/json");
      }
      if (tile) {
        const body = mvt(data.type, all, Number(tile[1]), Number(tile[3]), Number(tile[2]));
        return body ? r.fulfill({ status: 200, contentType: MVT, body }) : r.fulfill({ status: 204 });
      }
      const one = /\/items\/([^/]+)$/.exec(url.pathname);
      if (one) {
        const f = all.find((x) => x.id === decodeURIComponent(one[1]));
        return f ? send(JSON.stringify(f), "application/geo+json") : send("{}", "application/json", 404);
      }
      const bbox = url.searchParams.get("bbox");
      if (bbox) {
        const [w, s, e, n] = bbox.split(",").map(Number);
        all = all.filter((f) => { const [x, y] = f.geometry.coordinates.flat(2); return x >= w && x <= e && y >= s && y <= n; });
      }
      const limit = Number(url.searchParams.get("limit") || 10);
      const offset = Number(url.searchParams.get("offset") || 0);
      const features = all.slice(offset, offset + limit);
      const links = [];
      if (offset + limit < all.length) {
        const next = new URL(url);
        next.searchParams.set("offset", String(offset + limit));
        links.push({ rel: "next", href: next.href, type: "application/geo+json" });
      }
      return send(JSON.stringify({ type: "FeatureCollection", numberMatched: all.length, numberReturned: features.length, features, links }), "application/geo+json");
    }
    return r.abort();
  });
  return asked;
}

const NOTE = {
  points: /^$/, polygons: /^$/, many: /^Showing the first 5,000; zoom in to see the rest\.$/,
  tiled: /^Showing the first 10,000; zoom in to see the rest\.$/,
  empty: /^This dataset has no places to show on a map\.$/, failing: /^The map could not load its data/
};

for (const kind of Object.keys(DATA)) {
  for (const width of [1440, 400]) {
    for (const scheme of ["light", "dark"]) {
      test(`the catalogue map of ${kind} at ${width} px, ${scheme}`, async ({ page: p }) => {
        await p.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await p.emulateMedia({ colorScheme: scheme });
        const asked = await route(p, kind);
        await p.goto(`${CATALOGUE}/map/${kind}`);
        const box = p.locator(".jc-map");
        const note = p.locator(".jc-map-note");
        await expect(note).toHaveText(NOTE[kind], { timeout: 30000 });
        if (kind === "points" || kind === "polygons" || kind === "many" || kind === "tiled") {
          await expect(box).toHaveAttribute("data-state", "ready");
          await expect(p.locator(".jc-map-area canvas")).toBeVisible();
        }
        if (kind === "points" || kind === "many") await expect(p.locator(".jc-map-cluster").first()).toBeVisible();
        // The keys line is for screen readers; the page of its own shows only the map.
        await expect(p.locator("#jc-map-keys")).toHaveCSS("position", "absolute");
        if (kind === "many") {
          // Five pages of 1000, then nothing more until the view moves; a move asks for its bbox.
          const items = () => asked.filter((a) => a.url.pathname.endsWith("/items"));
          expect(items().filter((a) => !a.url.searchParams.has("bbox"))).toHaveLength(5);
          await p.evaluate(() => { const m = document.querySelector(".jc-map").jcMap; m.jumpTo({ center: [24.75, 60.12], zoom: 13 }); });
          await expect.poll(() => items().filter((a) => a.url.searchParams.has("bbox")).length).toBeGreaterThan(0);
          const bbox = items().find((a) => a.url.searchParams.has("bbox")).url.searchParams.get("bbox").split(",").map(Number);
          expect(bbox).toHaveLength(4);
          expect(bbox[0]).toBeLessThan(24.75);
          expect(bbox[2]).toBeGreaterThan(24.75);
          await expect(box).toHaveAttribute("data-state", "ready");
          // Then it rests: a note coming and going must not resize the map into another read.
          const settled = asked.length;
          await p.waitForTimeout(2000);
          expect(asked.length).toBe(settled);
        }
        if (kind === "tiled") {
          // One page for the first view, then tiles only: no bbox reads, no paging to the cap.
          await expect(box).toHaveAttribute("data-tiled", "true");
          const tiles = () => asked.filter((a) => /\/tiles\/WebMercatorQuad\/\d+\/\d+\/\d+$/.test(a.url.pathname));
          await expect.poll(() => tiles().length).toBeGreaterThan(0);
          for (const a of tiles()) expect(a.url.search).toBe("");
          await p.evaluate(() => { const m = document.querySelector(".jc-map").jcMap; m.jumpTo({ center: [24.75, 60.12], zoom: 15 }); });
          await expect.poll(() => p.evaluate(() => document.querySelector(".jc-map").jcMap.queryRenderedFeatures({ layers: ["jc-tiles-0-point"] }).length), { timeout: 15000 }).toBeGreaterThan(0);
          // Close in, no tile is cut: the cap note goes.
          await expect(note).toHaveText(/^$/);
          const items = asked.filter((a) => a.url.pathname.endsWith("/items"));
          expect(items.length).toBeLessThanOrEqual(2);
          expect(items.filter((a) => a.url.searchParams.has("bbox"))).toHaveLength(0);
          // A click on a tile's point opens the whole feature, read by its URN.
          const at = await p.evaluate(() => {
            const m = document.querySelector(".jc-map").jcMap;
            const f = m.queryRenderedFeatures({ layers: ["jc-tiles-0-point"] })[0];
            const q = m.project(f.geometry.coordinates);
            const r = m.getCanvas().getBoundingClientRect();
            return { x: r.left + q.x, y: r.top + q.y, id: f.properties.id };
          });
          await p.mouse.click(at.x, at.y);
          await expect(p.locator(".jc-map-panel h3")).toHaveText(/^Asema \d+$/);
          expect(asked.some((a) => a.url.pathname.endsWith("/items/" + encodeURIComponent(at.id)))).toBe(true);
        }
        for (const a of asked) {
          expect(a.headers.authorization).toBeUndefined();
          expect(a.headers.cookie).toBeUndefined();
          expect(a.url.pathname).not.toMatch(/datastore|\/api\/3\/action|\.geojson$|\.csv$/);
        }
        await p.screenshot({ path: path.join(shots, `catalogue-map-${kind}-${scheme}-${width}.png`), fullPage: true });
      });
    }
  }
}
