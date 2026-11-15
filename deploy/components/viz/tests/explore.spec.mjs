// T-5059, T-5082: the live explorer in a browser. The table is the Portal's Endpoint data view
// (images/viz/explorer, scripts/viz-explorer-bundle.sh); this page adds the address, the map and the
// Data API dialog. Combinations: viewer {anonymous, signed-in} x width {1440, 400} x language {en, sk}
// x data {point, polygon, no geometry, 0, 1, >10 000 with paging, long texts and URLs, multi-valued and
// LanguageProperty} x theme for two of them, the failures {403, 404, 502}, a shared link's query and
// order, the server-side order, Fields, full window and CSV. Every host is routed inside the test (viz
// with nginx.conf's CSP, the Endpoint with answers shaped like dev's of 2026-10-21), so it runs
// anywhere; each state leaves a screenshot in test-results/explore/ for the UX read in the task.
//   npm i --no-save --no-package-lock @playwright/test@1.63.0 (chromium 1243, as the Portal pins)
//   PLAYWRIGHT_BROWSERS_PATH=/ms-playwright npx playwright test -c components/viz/tests explore
import { test, expect } from "@playwright/test";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { shots, DOMAIN, VIZ, CATALOGUE, API, SLUGS, DATA, serve } from "./explore-hosts.mjs";

const frame = (page) => page.frameLocator("#explore");
const apiCalls = (requests) => requests.filter((r) => r.url.hostname === API);
const entityCalls = (requests) => apiCalls(requests).filter((r) => r.url.pathname.endsWith("/ngsi-ld/v1/entities") && r.headers.accept !== "application/geo+json");
const open = async (page, name, { lang = "en", theme, width = 1440, height = 900, extra = "" } = {}) => {
  await page.setViewportSize({ width, height });
  await page.goto(`https://${CATALOGUE}/dataset/${name}?lang=${lang}${theme ? "&theme=" + theme : ""}${extra ? "&extra=" + encodeURIComponent(extra) : ""}`);
};
const shot = (page, name) => page.screenshot({ path: path.join(shots, `${name}.png`), fullPage: true });
const rows = (f) => f.getByRole("grid").locator("tbody tr");

// What each data set shows once loaded: data rows in the grid, whether a map appears, a text it says.
const EXPECT = {
  point: { rows: 3, map: true }, polygon: { rows: 2, map: true }, nogeo: { rows: 4, map: false }, zero: { rows: 0, map: false, says: { en: /No entities/, sk: /Žiadne entity/ } },
  one: { rows: 1, map: true }, many: { rows: 20, map: false, says: { en: /12345 matching/, sk: /12345 vyhovujúcich/ } },
  long: { rows: 1, map: false }, multi: { rows: 1, map: false }, notypes: { rows: null, map: false }
};
const THEMED = new Set(["point", "many"]);

for (const name of Object.keys(EXPECT)) {
  for (const width of [1440, 400]) {
    for (const theme of THEMED.has(name) ? ["light", "dark"] : ["light"]) {
      for (const lang of ["en", "sk"]) {
        test(`${name} ${width} ${theme} ${lang}: shows the Endpoint's own rows and asks no other host`, async ({ page, context }) => {
          const errors = [];
          page.on("pageerror", (e) => errors.push(String(e)));
          const requests = await serve(context);
          await open(page, name, { lang, theme, width });
          const e = EXPECT[name];
          const f = frame(page);
          if (e.rows === null) {
            await expect(f.locator("#grid")).not.toBeEmpty();
            await expect(f.getByRole("grid")).toHaveCount(0);
          } else {
            await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
            if (e.rows > 0) await expect(rows(f)).toHaveCount(e.rows);
          }
          if (e.says) await expect(f.locator("#grid")).toContainText(e.says[lang]);
          await expect(f.locator("#map-section")).toBeVisible({ visible: e.map });
          if (e.map) await expect(f.locator("#map-note")).not.toBeEmpty();
          await expect(f.locator("html")).toHaveAttribute("lang", lang);
          expect(requests.filter((r) => ![VIZ, CATALOGUE, API, `tiles.${DOMAIN}`].includes(r.url.hostname)).map((r) => String(r.url))).toEqual([]);
          expect(errors).toEqual([]);
          if (e.map) {
            await f.locator("#map").scrollIntoViewIfNeeded();
            await expect(f.locator("#map .maplibregl-canvas")).toHaveCount(1, { timeout: 20000 });
            await page.waitForTimeout(500);
          }
          await shot(page, `${name}-${width}-${theme}-${lang}`);
        });
      }
    }
  }
}

for (const viewer of ["anonymous", "signed-in"]) {
  test(`${viewer}: the explorer sends no Authorization, no cookie and never asks the DataStore`, async ({ page, context }) => {
    if (viewer === "signed-in") {
      // The catalogue's session cookie and a cookie for the whole domain: neither may reach the API.
      await context.addCookies([{ name: "ckan", value: "session-xyz", domain: CATALOGUE, path: "/", secure: true, httpOnly: true },
        { name: "auth_tkt", value: "abc", domain: `.${DOMAIN}`, path: "/", secure: true }]);
    }
    const requests = await serve(context);
    await open(page, "point");
    await expect(rows(frame(page))).toHaveCount(3, { timeout: 20000 });
    const calls = apiCalls(requests);
    expect(calls.length).toBeGreaterThan(3);
    for (const c of calls) {
      expect(c.headers.authorization).toBeUndefined();
      expect(c.headers.cookie).toBeUndefined();
    }
    expect(requests.filter((r) => /datastore_/.test(r.url.pathname))).toEqual([]);
    await shot(page, `viewer-${viewer}`);
  });
}

// The surface's own sentence, as the Portal shows it.
const FAILED = { forbidden: /no policy grants anonymous reads/, missing: /does not exist/, gateway: /did not answer/ };
for (const name of Object.keys(FAILED)) {
  for (const [width, lang] of [[1440, "en"], [400, "sk"]]) {
    test(`${name} ${width} ${lang}: a failure says what happened`, async ({ page, context }) => {
      await serve(context);
      await open(page, name, { lang, width });
      await expect(frame(page).locator("#grid")).toContainText(FAILED[name], { timeout: 20000 });
      await expect(frame(page).getByRole("grid")).toHaveCount(0);
      await shot(page, `fail-${name}-${width}-${lang}`);
    });
  }
}

test("a shared link opens its query and order, and a header sorts every row through orderBy", async ({ page, context }) => {
  const requests = await serve(context);
  await open(page, "point", { extra: "&type=BikeHireDockingStation&q=" + encodeURIComponent('status=="working"') + "&sort=availableBikeNumber" });
  const f = frame(page);
  await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
  await expect.poll(() => entityCalls(requests).some((r) => r.url.searchParams.get("orderBy") === "availableBikeNumber" && r.url.searchParams.get("q") === 'status=="working"')).toBe(true);
  const sort = f.getByRole("button", { name: /Available bike number/ }).first();
  await sort.click();
  await expect.poll(() => entityCalls(requests).some((r) => r.url.searchParams.get("orderBy") === "availableBikeNumber;desc")).toBe(true);
  await expect.poll(async () => decodeURIComponent(await f.locator("html").evaluate(() => location.hash))).toContain("sort=availableBikeNumber;desc");
  await shot(page, "sorted-1440");
});

test("Data API lists the view's addresses, CSV with its query and order", async ({ page, context }) => {
  await serve(context);
  await open(page, "point", { extra: "&q=" + encodeURIComponent('status=="working"') + "&sort=availableBikeNumber;desc" });
  const f = frame(page);
  await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
  await f.locator("#data-api").click();
  const links = f.locator("#detail-links a");
  await expect(links).toHaveCount(6);
  const csv = await links.nth(3).getAttribute("href");
  expect(csv).toBe(`https://${API}/api/endpoint/${SLUGS.point}/file.csv?type=BikeHireDockingStation&q=status%3D%3D%22working%22&orderBy=availableBikeNumber%3Bdesc`);
  expect(await links.nth(4).getAttribute("href")).toBe(`https://${API}/api/endpoint/${SLUGS.point}/ogc/features/collections/BikeHireDockingStation/tiles/WebMercatorQuad`);
  expect(await links.nth(5).getAttribute("href")).toMatch(/explore\.html#.*sort=availableBikeNumber%3Bdesc/);
  await shot(page, "data-api-1440");
  await f.locator("#detail-close").click();
  await expect(f.locator("#data-api")).toBeFocused();
});

for (const width of [1440, 400]) {
  test(`${width}: Hide fields names each column; the full window opens and leaves by Escape`, async ({ page, context }) => {
    await serve(context);
    await open(page, "point", { width });
    const f = frame(page);
    await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
    // Hide fields names each column by its title (T-6303: the schema descriptions left with the Columns list).
    await f.locator("summary").filter({ hasText: /^Hide fields/ }).click();
    await expect(f.getByRole("checkbox", { name: /available ?bike ?number/i })).toBeInViewport();
    await shot(page, `fields-${width}`);
    await page.keyboard.press("Escape");
    await f.getByRole("button", { name: "Full window" }).click();
    await expect(f.getByRole("dialog", { name: /endpoint/i })).toBeVisible();
    await shot(page, `full-${width}`);
    await page.keyboard.press("Escape");
    await expect(f.getByRole("dialog", { name: /endpoint/i })).toHaveCount(0);
  });
}

test("paging walks a large Endpoint by offset with its count", async ({ page, context }) => {
  const requests = await serve(context);
  await open(page, "many");
  const f = frame(page);
  await expect(rows(f)).toHaveCount(20, { timeout: 20000 });
  await f.getByRole("button", { name: /Next/ }).first().click();
  await expect.poll(() => entityCalls(requests).some((r) => r.url.searchParams.get("offset") === "20")).toBe(true);
  await shot(page, "paged-1440");
});

test("the map asks the grid's query within its view as GeoJSON", async ({ page, context }) => {
  const requests = await serve(context);
  await open(page, "point", { extra: "&q=" + encodeURIComponent('status=="working"') });
  await expect(frame(page).locator("#map-section")).toBeVisible({ timeout: 20000 });
  await expect.poll(() => apiCalls(requests).some((r) => r.headers.accept === "application/geo+json" && r.url.searchParams.get("georel") === "within" && r.url.searchParams.get("q") === 'status=="working"')).toBe(true);
});

test("MapLibre and its tiles load only once the map is in the reader's window (T-5794)", async ({ page, context }) => {
  const requests = await serve(context);
  // A short phone window: the toolbar, the pager and the table's page push the map below the fold
  // (the owner's UI pass 9f0182ef9 made the table short; T-6303 folded the query block: ~565 px).
  await open(page, "point", { width: 400, height: 500 });
  const f = frame(page);
  await expect(rows(f)).toHaveCount(EXPECT.point.rows, { timeout: 20000 });
  await expect(f.locator("#map-section")).toBeVisible();
  // The frame takes its content's height from the frame's message.
  await expect.poll(() => page.locator("#explore").evaluate((e) => e.getBoundingClientRect().height), { timeout: 20000 }).toBeGreaterThan(500);
  const outerTop = await page.locator("#explore").evaluate((e) => e.getBoundingClientRect().top);
  const mapTop = await f.locator("#map").evaluate((m) => m.getBoundingClientRect().top);
  expect(outerTop + mapTop).toBeGreaterThan(500);
  const mapLib = () => requests.filter((r) => /maplibre-gl.*\.mjs$/.test(r.url.pathname) || r.url.hostname === `tiles.${DOMAIN}`);
  await page.waitForTimeout(1500);
  expect(mapLib().map((r) => String(r.url))).toEqual([]);
  await expect(f.locator("#map .maplibregl-canvas")).toHaveCount(0);
  // The reader scrolls down to the map.
  await page.evaluate((y) => window.scrollTo(0, y), outerTop + mapTop - 100);
  await expect(f.locator("#map .maplibregl-canvas")).toHaveCount(1, { timeout: 20000 });
  expect(mapLib().length).toBeGreaterThan(0);
});

test("a row's \"Show on the map\" before the map loaded opens the map at the place (T-5794)", async ({ page, context }) => {
  await serve(context);
  await open(page, "point", { width: 400 });
  const f = frame(page);
  await expect(rows(f)).toHaveCount(EXPECT.point.rows, { timeout: 20000 });
  await expect(f.locator("#map-section")).toBeVisible();
  await f.getByRole("button", { name: /^Open.*: Station 1$/ }).first().click();
  await expect(f.locator("#detail-title")).toHaveText("Station 1");
  await f.locator("#detail-onmap").click();
  await expect(f.locator("#map .maplibregl-canvas")).toHaveCount(1, { timeout: 20000 });
  const inner = page.frame({ url: /explore\.html/ });
  await expect.poll(() => inner.evaluate(() => (self.jcExploreMap ? self.jcExploreMap.getZoom() : 0)), { timeout: 20000 }).toBeGreaterThanOrEqual(15);
});

test("past 1000 in view the map draws the type's vector tiles; a point opens its entity (T-5189)", async ({ page, context }) => {
  for (const width of [1440, 400]) {
    const requests = await serve(context);
    await open(page, "tiled", { width });
    const f = frame(page);
    await expect(f.locator("#map-section")).toBeVisible({ timeout: 20000 });
    await f.locator("#map").scrollIntoViewIfNeeded();
    const tiles = () => apiCalls(requests).filter((r) => /\/tiles\/WebMercatorQuad\/\d+\/\d+\/\d+$/.test(r.url.pathname));
    await expect.poll(tiles, { timeout: 20000 }).not.toHaveLength(0);
    const inner = page.frame({ url: /explore\.html/ });
    await expect.poll(() => inner.evaluate(() => self.jcExploreMap.queryRenderedFeatures({ layers: ["jc-tile-point"] }).length), { timeout: 20000 }).toBeGreaterThan(0);
    await expect(f.locator("#map-note")).toHaveText(/Click a point/);
    for (const r of tiles()) {
      expect(r.url.search).toBe("");
      expect(r.headers.authorization).toBeUndefined();
      expect(r.headers.cookie).toBeUndefined();
    }
    await shot(page, `tiled-${width}`);
    const at = await inner.evaluate(() => {
      const m = self.jcExploreMap;
      const hit = m.queryRenderedFeatures({ layers: ["jc-tile-point"] })[0];
      const q = m.project(hit.geometry.coordinates);
      return { x: Math.round(q.x), y: Math.round(q.y) };
    });
    await f.locator("#map canvas").click({ position: at });
    await expect(f.locator("#detail-title")).toHaveText(/^Station \d$/);
    expect(apiCalls(requests).some((r) => /\/ngsi-ld\/v1\/entities\/urn%3Angsi-ld%3ABikeHireDockingStation/.test(r.url.pathname))).toBe(true);
    await shot(page, `tiled-detail-${width}`);
    await context.unrouteAll({ behavior: "ignoreErrors" });
  }
});

// DM-135 (T-5231): a row or a point opens one citizen card, centred in the slice of the frame the
// reader sees after scrolling the page, keyboard-trapped, closed by Esc or the backdrop.
for (const [width, height] of [[1440, 900], [400, 800]]) {
  for (const theme of ["light", "dark"]) {
    test(`${width} ${theme}: a row opens the card in the middle of what the reader sees, after a 1,500 px scroll`, async ({ page, context }) => {
      await serve(context);
      await open(page, "point", { width, theme });
      await page.setViewportSize({ width, height });
      const f = frame(page);
      await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
      // A long page around the frame, as a CMS article with the widget in its middle.
      await page.evaluate(() => {
        const frameEl = document.getElementById("explore");
        frameEl.insertAdjacentHTML("beforebegin", '<div style="height:1600px">Article</div>');
        frameEl.insertAdjacentHTML("afterend", '<div style="height:3000px">More</div>');
        window.scrollTo(0, 1500);
      });
      await page.waitForTimeout(400);
      const opener = f.getByRole("button", { name: /^Open.*: Station 1$/ }).first();
      await opener.focus();
      await opener.press("Enter");
      const card = f.locator("#detail");
      await expect(card).toBeVisible();
      await expect(f.locator("#detail-title")).toHaveText("Station 1");
      await expect(f.locator("#detail-type")).not.toHaveText("");
      await expect(f.locator("#detail-list")).not.toContainText("urn:ngsi-ld");
      await expect(f.locator("#detail-list")).not.toContainText("https://");
      await expect(f.locator("#detail-dev")).not.toHaveAttribute("open", "");
      await expect(f.locator("#detail-source")).toContainText("hsl.fi");
      await expect(f.locator("#detail-map")).toBeVisible();
      const where = await page.evaluate(() => {
        const fr = document.getElementById("explore").getBoundingClientRect();
        const top = Math.max(0, fr.top), bottom = Math.min(window.innerHeight, fr.bottom);
        return { frameTop: fr.top, frameLeft: fr.left, frameWidth: fr.width, slice: (top + bottom) / 2, inner: window.innerHeight };
      });
      const box = await card.boundingBox(); // page coordinates: Playwright adds the frame's offset
      expect(Math.abs(box.y + box.height / 2 - where.slice)).toBeLessThanOrEqual(24);
      expect(Math.abs(box.x + box.width / 2 - (where.frameLeft + where.frameWidth / 2))).toBeLessThanOrEqual(24);
      const close = await f.locator("#detail-close").boundingBox();
      expect(close.y).toBeGreaterThanOrEqual(0);
      expect(close.y + close.height).toBeLessThanOrEqual(where.inner);
      expect(close.height).toBeGreaterThanOrEqual(44);
      if (width === 400) expect(Math.round(box.width)).toBe(width - 32);
      await page.screenshot({ path: path.join(shots, `card-row-${width}-${theme}.png`) });
      // Tab stays inside the card.
      for (let i = 0; i < 8; i++) {
        await page.keyboard.press("Tab");
        expect(await f.locator("#detail").evaluate((d) => (d.contains(document.activeElement) ? "in" : document.activeElement.outerHTML.slice(0, 120)))).toBe("in");
      }
      await page.keyboard.press("Escape");
      await expect(card).toBeHidden();
      await expect(opener).toBeFocused();
    });
  }
}

test("a point opens the same card; the fold holds the id and the API links; the backdrop closes it", async ({ page, context }) => {
  await serve(context);
  await open(page, "point", { lang: "sk" });
  const f = frame(page);
  await expect(f.locator("#map-section")).toBeVisible({ timeout: 20000 });
  await f.locator("#map").scrollIntoViewIfNeeded();
  const inner = page.frame({ url: /explore\.html/ });
  await expect.poll(() => inner.evaluate(() => (self.jcExploreMap && self.jcExploreMap.getLayer("jc-point") ? self.jcExploreMap.queryRenderedFeatures({ layers: ["jc-point"] }).length : 0)), { timeout: 20000 }).toBeGreaterThan(0);
  const at = await inner.evaluate(() => {
    const m = self.jcExploreMap;
    const q = m.project(m.queryRenderedFeatures({ layers: ["jc-point"] })[0].geometry.coordinates);
    return { x: Math.round(q.x), y: Math.round(q.y) };
  });
  await f.locator("#map canvas").click({ position: at });
  await expect(f.locator("#detail-title")).toHaveText(/^Stanica \d$/);
  await expect(f.locator("#detail-onmap")).toHaveText("Zobraziť na mape");
  await f.locator("#detail-dev summary").click();
  await expect(f.locator("#detail-dev-list")).toContainText("urn:ngsi-ld:BikeHireDockingStation");
  await expect(f.locator("#detail-links a")).toHaveCount(6);
  for (const a of await f.locator("#detail-dev a, #detail-source a").all()) {
    expect(await a.getAttribute("rel")).toBe("noopener noreferrer");
    expect(await a.getAttribute("href")).toMatch(/^https:\/\//);
  }
  await page.screenshot({ path: path.join(shots, "card-point-1440-sk-dev.png") });
  await page.mouse.click(5, 5); // the backdrop, outside the card
  await expect(f.locator("#detail")).toBeHidden();
});

// T-5415: the owner's phone. At 400 px nothing scrolls the page sideways, the Data API / Embed
// buttons have one width, the type reads in Slovak from the model's label, the query hint quotes
// this type's own attribute, the Filter panel opens in the reader's window (T-6303, T-6319) and the
// pager is one line under its count.
for (const theme of ["light", "dark"]) {
  test(`400 ${theme} sk: one width per button, Slovak type, own query example, the Filter panel in view, one-line pager (T-5415)`, async ({ page, context }) => {
    await serve(context);
    await open(page, "stations", { lang: "sk", theme, width: 400 });
    const f = frame(page);
    await expect(f.getByRole("grid")).toBeVisible({ timeout: 20000 });
    const overflow = (doc) => doc.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    expect(await overflow(f.locator("html"))).toBeLessThanOrEqual(0);
    const select = f.locator("select[id^='endpoint-data-']");
    await expect(select).toHaveCount(1);
    const buttons = f.locator(".page-head .page-button:visible");
    const widths = await buttons.evaluateAll((all) => all.map((one) => Math.round(one.getBoundingClientRect().width)));
    expect(new Set(widths).size).toBe(1);
    await expect(select.locator("option:checked")).toHaveText("Stanica");
    // The query and its hint fold under the Query button (T-6303).
    await f.getByRole("button", { name: "Dopyt", exact: true }).click();
    await expect(f.locator(".jc-grid-query-hint code")).toHaveText('name=="Stanica 1"');
    await expect(f.locator(".jc-grid-query-value")).toHaveText("všetky záznamy");
    // The conditions open from the Filter button, under the toolbar and in the reader's window.
    await f.locator("summary").filter({ hasText: /^Filter/ }).click();
    await expect(f.getByRole("button", { name: "Pridať podmienku", exact: true })).toBeInViewport();
    expect(await overflow(f.locator("html"))).toBeLessThanOrEqual(0);
    // The dataset page shrinks the assistant's corner to its icon while the frame is in view (jc-viz.js).
    await expect(page.locator("body")).toHaveClass(/jc-corner-compact/);
    await shot(page, `phone-stations-400-${theme}`);
  });
}

test("400 sk: the pager of 12345 rows is one line of arrows and pages under its count (T-5415)", async ({ page, context }) => {
  await serve(context);
  await open(page, "many", { lang: "sk", width: 400 });
  const f = frame(page);
  await expect(rows(f)).toHaveCount(20, { timeout: 20000 });
  const tops = await f.locator(".jc-grid-pager button").evaluateAll((all) => all.map((one) => one.getBoundingClientRect().top));
  const middles = await f.locator(".jc-grid-pager button").evaluateAll((all) => all.map((one) => Math.round((one.getBoundingClientRect().top + one.getBoundingClientRect().bottom) / 2)));
  expect(new Set(middles).size).toBe(1);
  const total = await f.locator(".jc-grid-pager .jc-grid-total").boundingBox();
  expect(total.y).toBeLessThan(Math.min(...tops));
  await expect(f.locator(".jc-grid-pages [aria-current='page']")).toHaveText("1");
  await expect(f.getByRole("button", { name: "Nasledujúca" })).toBeVisible();
  await shot(page, "phone-pager-400");
});
