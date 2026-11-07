import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// The Slovak words of src/locales.ts, written out: that module imports the SDK, which Node does
// not load outside the bundle.
const BODIES = ["Banskobystrický samosprávny kraj", "Mesto Banská Bystrica"];

// UI-84, SDK-12 (T-2825): both bodies' indicators at a phone, a tablet, a laptop and a wall: no
// sideways scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
for (const size of WIDTHS) {
  test(`the indicators at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
    const { outside, missing, problems } = await serve(page);
    await page.setViewportSize(size);
    await page.goto(BASE);
    for (const body of BODIES) {
      await expect(page.getByRole("region", { name: body }).getByRole("article").first()).toBeVisible();
    }
    await testInfo.attach(`indicators-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
    expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
  });
}

// T-3386, SDK-40: the city's PM10 card opened in the SDK's entity panel by its territory, at a
// phone and a laptop, light and dark. The reader may only read, so the panel links to the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an indicator in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("region", { name: BODIES[1] }).getByRole("button", { name: "Mesto Banská Bystrica" }).first().click();
      const panel = page.getByRole("dialog", { name: "pm10-24h-mesto" });
      await expect(panel.getByText("KeyPerformanceIndicator")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
