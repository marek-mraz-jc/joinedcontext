import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-3140, T-3410): two datasets, the widest and the one of another space, each with
// and without a row in the SDK's entity panel, at a phone, a tablet, a laptop and a wall, light and
// dark: no sideways scroll, no two blocks over each other, nothing axe finds.
const VIEWS = [
  { dataset: "Kultúrne pamiatky", row: "Trojičný stĺp", open: false },
  { dataset: "Kultúrne pamiatky", row: "Trojičný stĺp", open: true },
  { dataset: "Publikácie UNIZA", row: "Conference paper", open: false },
];

for (const scheme of ["light", "dark"] as const) {
  for (const view of VIEWS) {
    for (const size of WIDTHS) {
      test(`${view.dataset}${view.open ? ", a row in the panel" : ""}, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { missing, problems, outside } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(BASE);
        await page.getByRole("navigation").getByRole("button", { name: new RegExp(view.dataset) }).click();
        await expect(page.getByRole("grid").getByText(view.row).first()).toBeVisible();
        if (view.open) {
          await page.getByRole("button", { name: `Otvoriť: ${view.row}` }).click();
          const panel = page.getByRole("dialog");
          await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
          await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
        }
        await testInfo.attach(`${view.dataset}-${view.open}-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        // The open panel is meant to lie over the page; the blocks under it are not the layout.
        expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
        expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
      });
    }
  }
}
