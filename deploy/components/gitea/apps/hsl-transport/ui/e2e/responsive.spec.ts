import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12 (T-2825, T-2926): the live map, its lines and its charts at a phone, a tablet, a laptop and a wall,
// light and dark: no sideways scroll, no two blocks over each other, no control cut off, nothing
// axe finds at WCAG 2.1 AA.
for (const scheme of ["light", "dark"] as const) {
  for (const size of WIDTHS) {
    test(`the buses, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { missing, problems } = await serve(page);
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize(size);
      await page.goto(BASE);
      await expect(page.getByRole("status")).toContainText("6 vehicles");
      await expect(page.getByRole("list", { name: "Departures from Tapiola (M)" })).toBeVisible();
      await expect(page.getByRole("list", { name: "Service alerts" })).toBeVisible();
      await expect(page.getByTestId("map").locator("canvas")).toHaveCount(1);
      await expect(page.getByRole("list", { name: "Lines on the map" }).getByText("1000N")).toBeVisible();
      // The two charts, coloured (T-2926): one bar per line in view, every speed band.
      const perLine = page.getByRole("list", { name: "Vehicles per line" });
      await expect(perLine.getByRole("listitem").first()).toBeVisible();
      await expect(page.getByRole("list", { name: "Speed" }).getByRole("listitem")).toHaveCount(7);
      await expect(perLine.getByTestId("bar-fill").first()).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
      await testInfo.attach(`buses-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect({ missing, problems }).toEqual({ missing: [], problems: [] });
    });
  }
}

// SDK-40, T-3399: a bus chosen from those in view opens in the shell's entity panel, read from the
// app's own backend, at a phone and a laptop, light and dark; the app writes nothing, so the panel
// links the bus to the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a bus in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      const { missing, problems } = await serve(page);
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("combobox", { name: "Open a vehicle" }).selectOption("3");
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("23", { exact: true })).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ missing, problems }).toEqual({ missing: [], problems: [] });
    });
  }
}
