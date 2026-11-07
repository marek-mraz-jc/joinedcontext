import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { CITY } from "../src/fixtures/records";
import { DATASETS } from "../src/labels";
import { BASE, serve } from "./serve";

// The Slovak words of src/locales.ts, written out: that module imports the SDK, which Node does
// not load outside the bundle.
const TITLE = "Záznamy mesta Banská Bystrica";
const NOTE_BOX = "Upraviť Poznámka správcu";
const REVIEW = "Skontrolovať zmeny";

// UI-84, SDK-12 (T-2825): the records and a note under review at a phone, a tablet, a laptop and
// a wall: no sideways scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
for (const view of ["the records", "a note under review", "the charts of a cube"]) {
  for (const size of WIDTHS) {
    test(`${view} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      await expect(page.getByRole("heading", { name: TITLE, level: 1 })).toBeVisible();
      await expect(page.locator("tbody").getByRole("row")).toHaveCount(CITY.length);
      if (view === "the charts of a cube") {
        // The water cube is the one the fixture holds (T-2966): its chart, named, not coded.
        await page.getByRole("button", { name: DATASETS.vh5003rr.sk }).click();
        await expect(page.getByRole("figure", { name: /Spotreba pitnej vody/ })).toBeVisible();
      }
      if (view === "a note under review") {
        await page.getByRole("textbox", { name: NOTE_BOX }).first().fill("Overené s odborom.");
        await page.getByRole("button", { name: REVIEW }).click();
        await expect(page.getByRole("region", { name: REVIEW })).toBeVisible();
      }
      await testInfo.attach(`${view}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3382, SDK-40: a record opened in the SDK's entity panel by its row's button, at a phone and a
// laptop, light and dark. Nobody is signed in here, so the panel links to the Portal and offers no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a record in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: "Otvoriť záznam" }).first().click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("KeyPerformanceIndicator")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
