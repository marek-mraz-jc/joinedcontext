import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2786): each dataset's grid at a phone, a tablet, a laptop and a wall: no
// sideways scroll of the page (the table scrolls in its own box), no overlap, axe clean.
const VIEWS = [
  { tab: "Místa", row: "Národní divadlo" },
  { tab: "Rozpočet", row: "Údržba komunikací" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.tab} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { missing, problems, outside } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      await page.getByRole("tab", { name: view.tab }).click();
      await expect(page.getByRole("tabpanel", { name: view.tab }).getByText(view.row)).toBeVisible();
      await testInfo.attach(`${view.tab}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
    });
  }
}

// SDK-40, T-3402: a row opened from the grid shows in the shell's entity panel, at a phone and a
// laptop, light and dark. A public App writes nothing, so the panel links to the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a place in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { missing, problems, outside } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: /: Národní divadlo$/ }).click();
      const panel = page.getByRole("dialog", { name: "Národní divadlo" });
      await expect(panel.getByRole("link", { name: "Otevřít v Portálu" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upravit" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
    });
  }
}
