import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test, expect, type Page } from "@playwright/test";

// A portal page's map and chart blocks (T-7433) in a browser, with the real jc-viz.js and
// jc-map.js: a map page that cannot load its data or has nothing to draw tells the block, and a
// chart frame that loads an error page gives up after ten seconds. No server is needed: every
// address is answered here. The block markup is templates/portal_page.html's map and chart arms.
//   node <portal ui>/node_modules/@playwright/test/cli.js test -c crates/dataroom/web/e2e portal_frames

const ASSETS = resolve(__dirname, "../assets");
const SITE = "https://opendata.city.test";
const VIZ = "https://viz.city.test";

const block = (kind: "map" | "chart", src: string) => {
  const frame =
    kind === "map"
      ? `<iframe class="jc-map-frame" title="Zastávky" src="${src}" sandbox="allow-scripts allow-same-origin"></iframe>`
      : `<iframe class="jc-viz-frame" title="Cestujúci" src="${src}" data-jc-viz="${VIZ}" sandbox="allow-scripts allow-same-origin"></iframe>`;
  return `<section class="jc-block jc-block-${kind}"><h2>${kind}</h2>
    <div class="jc-frame"><p class="jc-frame-state" aria-hidden="true" data-jc-failed="${kind} failed" data-jc-empty="${kind} empty">${kind} waiting</p>${frame}</div>
    <p class="jc-block-more"><a href="/dataset/x">Otvoriť dataset →</a></p></section>`;
};

const mapPage = (items: string) => {
  const config = JSON.stringify({
    t: { map_loading: "…", map_failed: "inner failed", map_empty: "inner empty" },
    layers: items ? [{ type: "Feature", items, schema: `${items}/schema` }] : [],
    lib: `${SITE}/maplibre.js`,
  });
  return `<!doctype html><div data-jc-map='${config}'><p class="jc-map-note"></p></div>
    <script type="module" src="/jc-map.js"></script>`;
};

async function serve(page: Page, home: string) {
  await page.route(`${SITE}/**`, async (route) => {
    const path = new URL(route.request().url()).pathname;
    const js = (name: string) =>
      route.fulfill({ contentType: "text/javascript", body: readFileSync(`${ASSETS}/public__${name}`) });
    if (path === "/") return route.fulfill({ contentType: "text/html", body: `<!doctype html><body>${home}<script src="/jc-viz.js"></script></body>` });
    if (path === "/jc-viz.js") return js("jc-viz.js");
    if (path === "/jc-map.js") return js("jc-map.js");
    if (path === "/maplibre.js") return route.fulfill({ contentType: "text/javascript", body: "export default {};" });
    if (path === "/map/broken") return route.fulfill({ contentType: "text/html", body: mapPage(`${SITE}/ogc/broken`) });
    if (path === "/map/empty") return route.fulfill({ contentType: "text/html", body: mapPage("") });
    if (path.startsWith("/ogc/broken")) return route.fulfill({ status: 503, body: "down" });
    return route.fulfill({ status: 404, body: "" });
  });
  // The chart host answers with an error page, which never says its height.
  await page.route(`${VIZ}/**`, (route) => route.fulfill({ status: 503, contentType: "text/html", body: "<h1>503</h1>" }));
  await page.goto(`${SITE}/`);
}

const state = (page: Page, kind: string) => page.locator(`.jc-block-${kind} .jc-frame-state`);

test("a map page that cannot load its data turns its block into the failure line", async ({ page }) => {
  await serve(page, block("map", `${SITE}/map/broken`));
  await expect(state(page, "map")).toHaveText("map failed");
  await expect(state(page, "map")).toHaveAttribute("role", "status");
  await expect(page.locator(".jc-map-frame")).toBeHidden();
});

test("a map page with nothing to draw turns its block into the no-data line", async ({ page }) => {
  await serve(page, block("map", `${SITE}/map/empty`));
  await expect(state(page, "map")).toHaveText("map empty");
  await expect(page.locator(".jc-map-frame")).toBeHidden();
});

test("a chart frame that never says its height gives up after ten seconds", async ({ page }) => {
  await serve(page, block("chart", `${VIZ}/#v1&inline`));
  await expect(state(page, "chart")).toHaveText("chart waiting");
  await expect(state(page, "chart")).toHaveText("chart failed", { timeout: 15000 });
  await expect(page.locator(".jc-viz-frame")).toBeHidden();
});
