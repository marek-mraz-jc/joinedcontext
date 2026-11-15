import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2825): every screen at a phone, a tablet, a laptop and a wall: no sideways
// scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "overview", hash: "#overview", choose: null },
  { name: "stations", hash: "#stations", choose: null },
  { name: "a chosen station", hash: "#stations", choose: "Kaivopuisto" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(`${BASE}${view.hash}`);
      if (view.hash === "#overview") {
        await expect(page.getByText("Bikes available", { exact: true })).toBeVisible();
        // T-2924: the map and the three charts are drawn on the first screen, in colour.
        const overview = page.getByRole("region", { name: "Overview" });
        await expect(overview.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
        await expect(overview.locator(".jc-chart-canvas canvas")).toHaveCount(3);
      } else {
        const stations = page.getByRole("region", { name: "Stations" });
        await expect(stations.getByRole("table").getByText("Kaivopuisto")).toBeVisible();
        await expect(stations.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
        if (view.choose) {
          await stations.getByRole("table").getByText(view.choose).click();
          // SDK-40: the station opens in the shell's entity panel, linked to the Portal, no Edit.
          const panel = page.getByRole("dialog", { name: view.choose });
          await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
          await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
        }
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// The panel in the dark as well, at a phone and a laptop, and Escape closes it (SDK-40, T-3395).
for (const width of [375, 1440]) {
  test(`dark at ${width} px: a station in the entity panel, axe clean, Escape closes it`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    const { outside, missing, problems } = await serve(page);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${BASE}#stations`);
    await page.getByRole("region", { name: "Stations" }).getByRole("table").getByText("Kaivopuisto").click();
    await expect(page.getByRole("dialog", { name: "Kaivopuisto" })).toBeVisible();
    expect(await layoutProblems(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
  });
}
