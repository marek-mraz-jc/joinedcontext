import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2784): each dataset's grid at a phone, a tablet, a laptop and a wall: no
// sideways scroll of the page (the table scrolls in its own box), no overlap, axe clean.
const VIEWS = [
  { tab: "Nemocnice", row: "Nemocnica Zvolen" },
  { tab: "Sociálne služby", row: "Domov sociálnych služieb Tisovec" },
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

// T-3383, SDK-40: a hospital opened in the SDK's entity panel by its name, at a phone and a laptop,
// light and dark. The App is public, so the panel links to it in the Portal and offers no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a row in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("tabpanel", { name: "Nemocnice" }).getByRole("button", { name: "Nemocnica Zvolen" }).click();
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
