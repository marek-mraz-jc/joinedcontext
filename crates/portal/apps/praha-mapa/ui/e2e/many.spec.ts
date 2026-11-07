import { expect, test } from "@playwright/test";
import { BASE, serve } from "./serve";

// T-5155: 12 000 sorting isles, three times what the map reads whole. Past the cap the map reads
// the isles inside its view (georel=within), and again after a pan or zoom.
const TOTAL = 12_000;
const ISLES = Array.from({ length: TOTAL }, (_, i) => {
  const lon = 14.25 + (i % 120) * 0.0035;
  const lat = 49.98 + Math.floor(i / 120) * 0.0019;
  return {
    id: `urn:ngsi-ld:WasteContainerIsle:praha.eu:praha-mesto:${i}`,
    type: "WasteContainerIsle",
    name: { type: "Property", value: `Stanoviště ${i}` },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } },
  };
});

for (const width of [1440, 400]) {
  test(`12 000 isles at ${width} px: the map reads the part in view and says so`, async ({ page }, testInfo) => {
    const { missing, problems, outside } = await serve(page);
    const areas: string[] = [];
    // Registered after serve's, so it answers the isles first.
    await page.route("**/api/endpoint/*/ngsi-ld/v1/entities?*", async (route) => {
      const params = new URL(route.request().url()).searchParams;
      if (params.get("type") !== "WasteContainerIsle") return route.fallback();
      let rows = ISLES;
      const coordinates = params.get("coordinates");
      if (params.get("georel") === "within" && coordinates) {
        areas.push(coordinates);
        const ring = (JSON.parse(coordinates) as number[][][])[0];
        const lons = ring.map((p) => p[0]);
        const lats = ring.map((p) => p[1]);
        const [w, e, s, n] = [Math.min(...lons), Math.max(...lons), Math.min(...lats), Math.max(...lats)];
        rows = ISLES.filter(({ location: { value: { coordinates: [x, y] } } }) => x >= w && x <= e && y >= s && y <= n);
      }
      const offset = Number(params.get("offset") ?? 0);
      const limit = Number(params.get("limit") ?? 20);
      return route.fulfill({ status: 200, contentType: "application/ld+json", body: JSON.stringify(rows.slice(offset, offset + limit)) });
    });
    await page.setViewportSize({ width, height: 900 });
    await page.goto(BASE);
    await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
    await expect(page.getByRole("status").filter({ hasText: /v zobrazené části mapy/ })).toBeVisible();
    expect(areas.length).toBeGreaterThan(0);
    await page.getByRole("checkbox", { name: /Tříděný odpad/ }).check();
    await testInfo.attach(`many-isles-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
    await page.screenshot({ path: testInfo.outputPath(`many-isles-${width}.png`), fullPage: true });
    expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
  });
}
