import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2782): the desk at a phone, a tablet, a laptop and a wall: the figures and the
// table, sorted, with no sideways scroll of the page (the table scrolls in its own region).
for (const size of WIDTHS) {
  test(`the school desk at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
    const { missing, problems, outside } = await serve(page);
    await page.setViewportSize(size);
    await page.goto(BASE);
    await expect(page.getByRole("table")).toBeVisible();
    await page.getByRole("button", { name: "Zoradiť podľa: Žiakov na učiteľa" }).click();
    await testInfo.attach(`desk-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
    expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
    expect({ missing, problems, outside }).toEqual({ missing: [], problems: [], outside: [] });
  });
}

// T-3381, SDK-40: a school opened in the SDK's entity panel by its name, at a phone and a laptop,
// light and dark. The reader may only read, so the panel links to it in the Portal and offers no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a school in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("table").getByRole("button", { name: "Základná škola, Tatranská 10" }).click();
      const panel = page.getByRole("dialog", { name: "Základná škola, Tatranská 10" });
      await expect(panel.getByText("Organization", { exact: true })).toBeVisible();
      await expect(panel.getByRole("link", { name: "Otvoriť v Portáli" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Upraviť" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
