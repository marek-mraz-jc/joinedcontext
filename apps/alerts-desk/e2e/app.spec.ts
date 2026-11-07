import { expect, test, type Page } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// SDK-12, UI-84 (T-2827): every page the application offers, at a phone, a tablet, a laptop and
// a wall: no sideways scroll, no two blocks over each other, no control cut off, nothing axe finds
// at WCAG 2.1 AA, and every page rendered from the fixtures without an error. The build lane runs
// this on every build, and a red width fails the build.

/** Waits until the page has read its data and drawn its charts. */
async function settled(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle");
  await expect(page.locator(".jc-loading")).toHaveCount(0);
  const charts = page.locator(".jc-chart-canvas");
  for (let i = 0; i < (await charts.count()); i++) await expect(charts.nth(i).locator("canvas")).toBeVisible();
}

for (const size of WIDTHS) {
  test(`every page at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
    const served = await serve(page);
    await page.setViewportSize(size);
    await page.goto(BASE);
    await settled(page);
    // The pages are the application's own navigation; one without any is checked as it opens.
    const tabs = page.locator("header nav").first().getByRole("button");
    const count = await tabs.count();
    for (let i = 0; i < Math.max(count, 1); i++) {
      let name = "home";
      if (count > 0) {
        name = (await tabs.nth(i).innerText()).trim() || `page ${i + 1}`;
        await tabs.nth(i).click();
        await settled(page);
      }
      await testInfo.attach(`${name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await page.locator(".jc-problem").allInnerTexts(), `${name}: an error on screen`).toEqual([]);
      expect(await layoutProblems(page, LIVE_BLOCKS), `${name} at ${size.width} px`).toEqual([]);
    }
    expect(served).toEqual({ outside: [], missing: [], problems: [] });
  });
}

// T-3376, SDK-40: an alert opened in the SDK's entity panel from its row, at a phone and a laptop,
// light and dark. The desk writes nothing, so the panel links to the alert in the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an alert in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await settled(page);
      await page.getByRole("row").filter({ hasText: "Mannerheimintie resurfacing" }).click();
      const panel = page.getByRole("dialog", { name: "Mannerheimintie resurfacing" });
      await expect(panel.getByText("One lane closed northbound.")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
