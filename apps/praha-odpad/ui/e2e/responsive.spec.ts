import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2786, T-3405): the desk, and a container opened in the SDK's entity panel, at a
// phone, a tablet, a laptop and a wall, light and dark: the figures and the table, sorted, with no
// sideways scroll of the page (the table scrolls in its own region), nothing axe finds.
for (const scheme of ["light", "dark"] as const) {
  for (const open of [false, true]) {
    for (const size of WIDTHS) {
      test(`${open ? "a container opened" : "the waste desk"}, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { missing, problems, outside } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(BASE);
        await expect(page.getByRole("table")).toBeVisible();
        await page.getByRole("button", { name: "Seřadit podle: Měřeno" }).click();
        if (open) {
          await page.getByRole("button", { name: "0001-PAP" }).click();
          const panel = page.getByRole("dialog");
          await expect(panel.getByRole("link", { name: "Otevřít v Portálu" })).toBeVisible();
          await expect(panel.getByRole("button", { name: "Upravit" })).toHaveCount(0);
        }
        await testInfo.attach(`${open ? "open" : "desk"}-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
        expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
      });
    }
  }
}
