import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2788): the places and a picked place in the shell's entity panel (SDK-40, T-3398)
// at a phone, a tablet, a laptop and a wall: no sideways scroll, no two blocks over each other,
// nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "the places", pick: null },
  { name: "a picked library", pick: "Keskustakirjasto Oodi" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { missing, problems, outside } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const results = page.getByRole("region", { name: /paikka/ });
      await expect(results.getByRole("button", { name: "Yrjönkadun uimahalli" })).toBeVisible();
      await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      if (view.pick) {
        await results.getByRole("button", { name: view.pick }).click();
        const panel = page.getByRole("dialog", { name: view.pick });
        await expect(panel.getByRole("link", { name: "Avaa portaalissa" })).toBeVisible();
        await expect(panel.getByRole("button", { name: "Muokkaa" })).toHaveCount(0);
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      // An open panel is meant to lie over the page on a phone; the blocks under it are not the layout.
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
    });
  }
}

// The panel in the dark as well, at a phone and a laptop, and Escape closes it.
for (const width of [375, 1440]) {
  test(`dark at ${width} px: a place in the entity panel, axe clean, Escape closes it`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    const { missing, problems, outside } = await serve(page);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(BASE);
    await page.getByRole("region", { name: /paikka/ }).getByRole("button", { name: "Hietaniemi, vesi" }).click();
    await expect(page.getByRole("dialog", { name: "Hietaniemi, vesi" })).toBeVisible();
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
  });
}
