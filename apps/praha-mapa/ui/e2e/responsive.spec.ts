import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2786, T-3403): the places, and a picked place in the SDK's entity panel, at a
// phone, a tablet, a laptop and a wall, light and dark: no sideways scroll, no two blocks over
// each other, nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "the places", pick: null },
  { name: "a picked theatre", pick: "Národní divadlo" },
];

for (const scheme of ["light", "dark"] as const) {
  for (const view of VIEWS) {
    for (const size of WIDTHS) {
      test(`${view.name}, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { missing, problems, outside } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(BASE);
        const results = page.getByRole("region", { name: /míst/ });
        await expect(results.getByRole("button", { name: /Veřejné WC Anděl/ })).toBeVisible();
        await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
        if (view.pick) {
          await results.getByRole("button", { name: new RegExp(view.pick) }).click();
          const panel = page.getByRole("dialog");
          await expect(panel.getByRole("link", { name: "Otevřít v Portálu" })).toBeVisible();
          await expect(panel.getByRole("button", { name: "Upravit" })).toHaveCount(0);
        }
        await testInfo.attach(`${view.name}-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        // The open panel is meant to lie over the page; the blocks under it are not the layout.
        expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
        expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
      });
    }
  }
}
