import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2825): the stations and a picked station's day at a phone, a tablet, a laptop
// and a wall: no sideways scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "the stations", pick: null },
  { name: "a picked station", pick: "Stanica 2" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const stations = page.getByRole("region", { name: "Stanice" });
      await expect(stations.getByRole("heading", { name: "Stanica 1" })).toBeVisible();
      await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      if (view.pick) {
        await stations.getByRole("article").filter({ has: page.getByRole("heading", { name: view.pick }) }).getByRole("button", { name: "Zobraziť stanicu" }).click();
        await expect(page.getByRole("region", { name: new RegExp(view.pick) })).toBeVisible();
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ missing, problems }).toEqual({ missing: [], problems: [] });
    });
  }
}

// T-3380, SDK-40: a station opened in the SDK's entity panel by its card's Details button, at a
// phone and a laptop, light and dark. The App is public, so the panel links to the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a station in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: "Podrobnosti: Stanica 1" }).click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("AirQualityObserved")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ missing, problems }).toEqual({ missing: [], problems: [] });
    });
  }
}
