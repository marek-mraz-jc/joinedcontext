import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve } from "./serve";

// UI-84, SDK-12, AP-138 (T-2923): the events page at a phone, a tablet, a laptop and a wall, with
// its two charts and its map drawn: no sideways scroll, no two blocks over each other, nothing axe
// finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "events", search: null },
  { name: "a search", search: "jazz" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(`${BASE}#events`);
      const events = page.getByRole("region", { name: "Events" });
      await expect(events.getByRole("list", { name: "Upcoming events" }).getByRole("listitem").first()).toBeVisible();
      await expect(events.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      await expect(events.locator(".jc-chart-canvas canvas")).toHaveCount(2);
      if (view.search) {
        await events.getByRole("searchbox", { name: "Search" }).fill(view.search);
        await expect(events.getByRole("heading", { name: "Jazz at Stoa" })).toBeVisible();
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// SDK-40, T-3397: an event opens in the shell's entity panel, read through the app's endpoint, at a
// phone and a laptop, light and dark; a public App writes nothing, so the panel links to the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an event in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      const { outside, missing, problems } = await serve(page);
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}#events`);
      await page.getByRole("list", { name: "Upcoming events" }).getByRole("button", { name: "Organ concert" }).click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("Unioninkatu 29, Helsinki")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
