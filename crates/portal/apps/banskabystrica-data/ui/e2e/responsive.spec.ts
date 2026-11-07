import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2782): each dataset's grid at a phone, a tablet, a laptop and a wall: no
// sideways scroll of the page (the table scrolls in its own box), no overlap, axe clean.
const VIEWS = [
  { tab: "Podujatia", row: "Radvanský jarmok" },
  { tab: "Školy", row: "Základná škola, Moyzesova 18" },
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

// T-3378, SDK-40, AP-140: an event opened in the SDK's entity panel from its name, at a phone and a
// laptop, light and dark. A public App: the panel reads it and links to it in the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an event in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { missing, problems, outside } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: "Otvoriť: Radvanský jarmok" }).click();
      const panel = page.getByRole("dialog", { name: "Radvanský jarmok" });
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toHaveAttribute("href", /entityId=urn%3Angsi-ld%3AEvent/);
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await panel.getByRole("button", { name: "Zavrieť" }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
    });
  }
}
