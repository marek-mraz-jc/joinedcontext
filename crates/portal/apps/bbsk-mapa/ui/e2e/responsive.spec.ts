import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2784): the places and a picked place in the SDK's entity panel at a phone, a tablet, a laptop and
// a wall: no sideways scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "the places", pick: null },
  { name: "a picked hospital", pick: "Nemocnica Zvolen" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { missing, problems, outside } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const results = page.getByRole("region", { name: /miest/ });
      await expect(results.getByRole("button", { name: /Stredoslovenské múzeum/ })).toBeVisible();
      await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      if (view.pick) {
        await results.getByRole("button", { name: new RegExp(view.pick) }).click();
        await expect(page.getByRole("dialog", { name: view.pick })).toBeVisible();
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
    });
  }
}

// T-3384, SDK-40: a hospital opened in the SDK's entity panel from the list, at a phone and a
// laptop, light and dark. The App is public, so the panel links to it in the Portal and offers no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a place in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("region", { name: /miest/ }).getByRole("button", { name: /Nemocnica Zvolen/ }).click();
      const panel = page.getByRole("dialog", { name: "Nemocnica Zvolen" });
      await expect(panel.getByText("Kuzmányho nábrežie 28, Zvolen")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
