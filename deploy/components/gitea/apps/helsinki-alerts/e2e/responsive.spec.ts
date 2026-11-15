import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { STEWARD, VIEWER } from "../src/fixtures/access";
import { serve } from "./serve";

// UI-84, SDK-12 (T-2825): every screen at a phone, a tablet, a laptop and a wall: no sideways
// scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA. The steward's views
// carry the record form, the viewer's only what they may read.
const VIEWS = [
  { name: "overview", role: "viewer", access: VIEWER, open: "overview" },
  { name: "alerts", role: "viewer", access: VIEWER, open: "list" },
  { name: "a chosen alert", role: "viewer", access: VIEWER, open: "detail" },
  { name: "the steward's new alert form", role: "steward", access: STEWARD, open: "new" },
  { name: "the steward's edit in the panel", role: "steward", access: STEWARD, open: "panel" },
  { name: "the steward's names and place form", role: "steward", access: STEWARD, open: "edit" },
] as const;

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      await page.setViewportSize(size);
      const { alerts, outside, problems } = await serve(page, view.role, view.access);
      await expect(alerts.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      if (view.open === "overview") {
        await page.goto("http://portal.test/#overview");
        await expect(page.getByRole("heading", { name: "Summary" })).toBeVisible();
      } else if (view.open === "new") {
        await alerts.getByRole("button", { name: "New alert" }).click();
        await expect(alerts.getByRole("form", { name: "New alert" })).toBeVisible();
      } else if (view.open !== "list") {
        await alerts.getByRole("table").getByText("Mannerheimintie resurfacing").click();
        const panel = page.getByRole("dialog");
        if (view.open === "edit") {
          // On a phone the panel covers the page: closed, the alert's actions stay above the map.
          await page.keyboard.press("Escape");
          await alerts.getByRole("button", { name: "Correct names and place" }).click();
          await expect(alerts.getByRole("form", { name: "Edit Alert" })).toBeVisible();
        } else if (view.open === "panel") {
          await panel.getByRole("button", { name: "Edit" }).click();
          await expect(panel.getByRole("button", { name: "Review the change" })).toBeVisible();
        } else {
          await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
        }
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, problems }).toEqual({ outside: [], problems: [] });
    });
  }
}

// SDK-40, T-3394: an alert in the entity panel, light and dark, at a phone and a laptop: a viewer
// is linked to the Portal, a steward gets Edit; Escape closes it.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an alert in the entity panel, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width, height: 900 });
      const { alerts, outside, problems } = await serve(page, "steward", STEWARD);
      await alerts.getByRole("table").getByText("Mannerheimintie resurfacing").click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByRole("button", { name: "Edit" })).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(panel).toHaveCount(0);
      expect({ outside, problems }).toEqual({ outside: [], problems: [] });
    });
  }
}
