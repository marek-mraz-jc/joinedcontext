import { chromium, expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// The basemap style is stubbed as well as the API: a journey must not depend on a tile
// service being up, and an empty style renders a real MapLibre canvas all the same.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "ovzdusie-prehlad", namespace: "banskabystrica" },
  spec: {
    title: { sk: "Kvalita ovzdušia", en: "Air quality" },
    visibility: "public",
    pages: [{ title: "Mapa staníc", layout: "full-map", layers: ["air-quality-stations"] }],
  },
  status: { phase: "Live" },
};

const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "air-quality-stations", namespace: "banskabystrica" },
  spec: {
    sourceEndpointRef: "public-air",
    entityType: "AirQualityObserved",
    style: "circle",
    colorBy: { property: "pm10", palette: "YlOrRd", domain: [0, 100] },
    sizeBy: { property: "pm10", range: [4, 18] },
    popupProperties: ["stationName", "pm10"],
  },
  status: { phase: "Live" },
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "public-air", namespace: "banskabystrica" },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "geojson"],
  },
  status: { phase: "Live" },
};

const FEATURES = {
  type: "FeatureCollection",
  features: [
    {
      type: "Feature",
      geometry: { type: "Point", coordinates: [19.146, 48.736] },
      properties: { stationName: "Štiavničky", pm10: 42 },
    },
  ],
};

test.describe("map dashboard", () => {
  test("renders the air quality dashboard from its manifests", async ({ page }) => {
    const geojsonRequests: string[] = [];

    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ version: 8, sources: {}, layers: [] }),
      }),
    );
    await page.route("**/api/endpoint/**", (route) => {
      geojsonRequests.push(new URL(route.request().url()).pathname);
      return route.fulfill({ contentType: "application/json", body: JSON.stringify(FEATURES) });
    });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items,
      });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards")) return json(list([DASHBOARD]));
      if (path.endsWith("/layers")) return json(list([LAYER]));
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      return json(list([]));
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=en");

    await expect(page.getByRole("heading", { name: "Air quality" })).toBeVisible();
    await expect(page.getByText("air-quality-stations")).toBeVisible();
    await expect(page.getByText(/coloured by pm10/)).toBeVisible();

    // Either the canvas is up or the page said WebGL is missing; a blank map is not allowed.
    const map = page.getByRole("application", { name: "Air quality" });
    const missing = page.getByText("The map needs WebGL");
    await expect(map.or(missing).first()).toBeVisible();

    if (await map.isVisible()) {
      await expect(map.locator("canvas")).toBeVisible();
    }
    // Whether MapLibre fetches the features from its worker is MapLibre's business; that the
    // view hands it the right Endpoint URL is asserted in tests/maplibre_view.test.tsx.
    expect(geojsonRequests.every((path) => path.startsWith(`/api/endpoint/${SLUG}/`))).toBe(true);
  });

  test("the dashboards route has no axe violations", async ({ page }) => {
    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ version: 8, sources: {}, layers: [] }),
      }),
    );
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [DASHBOARD] });
      if (path.endsWith("/layers"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [LAYER] });
      if (path.endsWith("/endpoints"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [ENDPOINT] });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=sk");
    await expect(page.getByRole("heading", { name: "Kvalita ovzdušia" })).toBeVisible();

    expect(await axeViolations(page)).toEqual([]);
  });
});

// A `grid` widget of a page, drawn by the same component the explorer renders (T-1440; UI-71,
// SDK-30): the manifest carries its configuration and the endpoint answers the rows.
const WITH_GRID = {
  ...DASHBOARD,
  spec: {
    ...DASHBOARD.spec,
    pages: [
      {
        title: "Data",
        layout: "grid-2x2",
        widgets: [
          {
            widgetType: "grid",
            endpointRef: "public-air",
            entityType: "AirQualityObserved",
            grid: { columns: [{ attr: "pm10", format: "number" }], pageSize: 25 },
          },
        ],
      },
    ],
  },
};

const STATIONS = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:bb:ovzdusie:001",
    type: "AirQualityObserved",
    pm10: { type: "Property", value: 42, unitCode: "GQ", observedAt: "2026-09-19T08:00:00Z" },
  },
];

test.describe("a grid widget of a dashboard", () => {
  test("draws the endpoint's entities from the manifest's own configuration", async ({ page }) => {
    const reads: string[] = [];
    await page.route("**/api/endpoint/**", (route) => {
      const url = new URL(route.request().url());
      reads.push(`${url.pathname}${url.search}`);
      return route.fulfill({
        contentType: "application/json",
        headers: { "NGSILD-Results-Count": "1" },
        body: JSON.stringify(STATIONS),
      });
    });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items,
      });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards")) return json(list([WITH_GRID]));
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      return json(list([]));
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=en");

    await expect(page.getByRole("grid")).toBeVisible();
    // The value with its unit's symbol, as the grid writes it since T-2812 (DM-06).
    await expect(page.getByText("42 µg/m³")).toBeVisible();
    // The widget's own endpoint and type, never another's: the slug is the endpoint it names.
    expect(reads.some((read) => read.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities?`))).toBe(
      true,
    );
    expect(reads.some((read) => read.includes("type=AirQualityObserved"))).toBe(true);
  });
});

// A page with the map and a grid widget: both draw MapLibre code, from one shared chunk
// (T-5514: the grid bundled the SDK's maplibre-gl 6.9.0 and the map the UI's 6.7.0, so this
// page downloaded MapLibre twice).
const MAP_AND_GRID = {
  ...DASHBOARD,
  spec: {
    ...DASHBOARD.spec,
    pages: [{ ...DASHBOARD.spec.pages[0], widgets: WITH_GRID.spec.pages[0].widgets }],
  },
};

test.describe("a page with the map and a grid", () => {
  test("loads MapLibre once for both", async ({ page }, testInfo) => {
    const scripts: string[] = [];
    page.on("request", (request) => {
      const path = new URL(request.url()).pathname;
      if (request.resourceType() === "script" && path.startsWith("/assets/")) scripts.push(path);
    });
    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ version: 8, sources: {}, layers: [] }),
      }),
    );
    await page.route("**/api/endpoint/**", (route) => {
      const geojson = new URL(route.request().url()).pathname.includes("geojson");
      return route.fulfill({
        contentType: "application/json",
        headers: { "NGSILD-Results-Count": "1" },
        body: JSON.stringify(geojson ? FEATURES : STATIONS),
      });
    });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items,
      });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards")) return json(list([MAP_AND_GRID]));
      if (path.endsWith("/layers")) return json(list([LAYER]));
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      return json(list([]));
    });

    await page.goto("/projects/banskabystrica/dashboards?lang=en");

    await expect(page.getByRole("grid")).toBeVisible();
    await expect(page.getByText("42 µg/m³")).toBeVisible();
    const map = page.getByRole("application", { name: "Air quality" });
    await expect(map.or(page.getByText("The map needs WebGL")).first()).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("map-and-grid.png"), fullPage: true });

    const maplibre = scripts.filter((path) => /\/maplibre-gl-(?!worker)[^/]*\.js$/.test(path));
    expect(maplibre).toHaveLength(1);
  });
});

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`a pan keeps the map the reader moved while the layer refetches (F-UI3-017, ${label})`, async ({ baseURL }) => {
    // Software WebGL, so a headless runner draws the map instead of skipping.
    const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
    const page = await (await browser.newContext({ baseURL })).newPage();
    test.info().attach("browser", { body: browser.version() });
    const reads: string[] = [];
    await page.setViewportSize({ width, height });
    await page.route("https://demotiles.maplibre.org/**", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ version: 8, sources: {}, layers: [] }) }),
    );
    await page.route("**/api/endpoint/**", async (route) => {
      reads.push(route.request().url());
      // A refetch slower than a frame: before F-UI3-017 the map gave way to the placeholder here.
      if (reads.length > 1) await new Promise((resolve) => setTimeout(resolve, 400));
      return route.fulfill({ contentType: "application/json", body: JSON.stringify(FEATURES) });
    });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/dashboards")) return json(list([DASHBOARD]));
      if (path.endsWith("/layers")) return json(list([LAYER]));
      if (path.endsWith("/endpoints")) return json(list([ENDPOINT]));
      return json(list([]));
    });
    await page.goto("/projects/banskabystrica/dashboards?lang=en");
    const map = page.getByRole("application", { name: "Air quality" });
    await map.or(page.getByText("The map needs WebGL")).first().waitFor();
    test.skip(!(await map.isVisible()), "no WebGL in this browser");
    await expect(map.locator("canvas")).toBeVisible();
    await map.evaluate((node) => node.setAttribute("data-first-mount", "yes"));
    const before = reads.length;

    const box = (await map.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 - 120, box.y + box.height / 2 - 60, { steps: 8 });
    await page.mouse.up();
    await expect.poll(() => reads.length).toBeGreaterThan(before);
    // While the refetch is in flight, and after it: the same map element, never the placeholder.
    await expect(page.getByRole("application", { name: "Air quality" })).toHaveAttribute("data-first-mount", "yes");
    await page.waitForTimeout(600);
    await expect(page.getByRole("application", { name: "Air quality" })).toHaveAttribute("data-first-mount", "yes");
    await page.screenshot({ path: test.info().outputPath(`dashboard-after-pan-${label}.png`), fullPage: true });
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/dashboard-after-pan-${label}.png`, fullPage: true });
    await browser.close();
  });
}
