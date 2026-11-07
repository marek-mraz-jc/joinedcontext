import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-3404): the city right now, and a car park opened in the SDK's entity panel, at a
// phone, a tablet, a laptop and a wall, light and dark: no sideways scroll, no two blocks over each
// other, nothing axe finds at WCAG 2.1 AA.
for (const scheme of ["light", "dark"] as const) {
  for (const open of [false, true]) {
    for (const size of WIDTHS) {
      test(`${open ? "a car park opened" : "the city right now"}, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { missing, problems, outside } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(BASE);
        const parks = page.getByRole("region", { name: "Parkoviště P+R" });
        await expect(parks.getByRole("button", { name: "P+R Běchovice" })).toBeVisible();
        await expect(page.getByRole("region", { name: "Kvalita ovzduší" }).getByRole("button", { name: "Praha 4-Libuš" })).toBeVisible();
        if (open) {
          await parks.getByRole("button", { name: "P+R Běchovice" }).click();
          const panel = page.getByRole("dialog");
          await expect(panel.getByRole("link", { name: "Otevřít v Portálu" })).toBeVisible();
          await expect(panel.getByRole("button", { name: "Upravit" })).toHaveCount(0);
        }
        await testInfo.attach(`${open ? "open" : "now"}-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
        expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
      });
    }
  }
}
