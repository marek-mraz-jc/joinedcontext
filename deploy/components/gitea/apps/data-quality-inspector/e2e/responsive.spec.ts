import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve, STEWARD, VIEWER } from "./serve";

for (const scheme of ["light", "dark"] as const) {
  for (const lang of ["fi", "en"] as const) {
    for (const size of WIDTHS) {
      test(`${scheme} ${lang} at ${size.width} px: types list has buttons, chart canvas drawn, choose BikeHireDockingStation shows failing table, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { outside, missing, problems } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(`${BASE}?lang=${lang}`);

        // The types list has buttons
        const typeButtons = page.locator(".app-type-btn");
        await expect(typeButtons).toHaveCount(4);

        // The chart canvas is drawn
        await expect(page.locator(".jc-chart-canvas")).toHaveCount(1);
        await expect(page.locator(".jc-chart-canvas canvas").first()).toBeVisible();

        // Choose BikeHireDockingStation and the failing table has rows
        await page.getByRole("button", { name: "BikeHireDockingStation", exact: true }).click();
        const failingTable = page.locator(
          lang === "fi" ? "table[aria-label='Virheelliset kohteet']" : "table[aria-label='Failing entities']",
        );
        await expect(failingTable).toBeVisible();
        await expect(failingTable.locator("tbody tr")).toHaveCount(4);

        await testInfo.attach(`${scheme}-${lang}-${size.width}.png`, {
          body: await page.screenshot({ fullPage: true }),
          contentType: "image/png",
        });

        // Sideways scroll, overlap, cut-off controls and what axe finds at WCAG 2.1 AA.
        expect(await layoutProblems(page)).toEqual([]);
        expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
      });
    }
  }
}

test("clicking back button returns to overview tiles and updates hash to #quality", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en#quality?type=BikeHireDockingStation`);

  await expect(page.getByRole("button", { name: "BikeHireDockingStation", exact: true })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "← All types" }).click();
  await expect(page.locator(".jc-tiles .jc-tile")).toHaveCount(4);
  expect(new URL(page.url()).hash).toBe("#quality");
});

// T-3354: the runs the App's daily job keeps (T-3579: no run on request), and a run's full report
// downloaded from the store.
test("the kept runs are listed and a run's full report opens", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  const trend = page.getByRole("table", { name: "Quality over time: the runs the server keeps" });
  await expect(trend.getByRole("row")).toHaveCount(3);
  await expect(page.getByRole("button", { name: "Inspect now" })).toHaveCount(0);
  await page.context().route("http://portal.test/store/**", (route) => route.fulfill({ status: 200, contentType: "application/json", body: "{}" }));
  const popup = page.waitForEvent("popup");
  await trend.getByRole("button", { name: /^Download the full report of the run at 21 Oct 2030/ }).click();
  const tab = await popup;
  await tab.waitForLoadState();
  expect(tab.url()).toBe("http://portal.test/store/runs/2/report.json");
  await tab.close();
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

test("all types failing displays retryable problem message", async ({ page }) => {
  await serve(page, []);
  await page.goto(`${BASE}?lang=en`);
  // All types have 0 entities with empty rows, Vehicle has no schema
  await expect(page.locator(".jc-tiles")).toBeVisible();
});

// T-3391, SDK-40: a failing entity opened in the SDK's entity panel by its id, at a phone and a
// laptop, light and dark. The App is public, so the panel links to the Portal and offers no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a failing entity in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await page.getByRole("button", { name: "BikeHireDockingStation", exact: true }).click();
      await page.getByRole("table", { name: "Failing entities" }).getByRole("button", { name: "fail-neg" }).first().click();
      // The panel names the entity by its name once read, by its id until then.
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("BikeHireDockingStation")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3827: a viewer sees the steward's buttons closed with the reason; a steward corrects a value,
// which resolves the finding, and finds it in the history. At a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer finds the steward's buttons closed, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en#quality?type=BikeHireDockingStation`);
      await expect(page.getByLabel("Your role: Viewer")).toBeVisible();
      const resolve = page.getByRole("button", { name: "Resolve: availableBikeNumber fail-neg" });
      await expect(resolve).toBeDisabled();
      await expect(resolve).toHaveAttribute("title", "Only a steward resolves or corrects findings.");
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward corrects a value and finds it in the history, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en#quality?type=BikeHireDockingStation`);
      await expect(page.getByLabel("Your role: Steward")).toBeVisible();
      await page.getByRole("button", { name: "Resolve: availableBikeNumber fail-neg" }).click();
      const form = page.getByRole("form", { name: "Resolve: availableBikeNumber of fail-neg" });
      await form.getByRole("textbox", { name: "Note" }).fill("Counted on site");
      await form.getByRole("combobox", { name: "Kind of value" }).selectOption("number");
      await form.getByRole("textbox", { name: "Correct value" }).fill("4");
      await testInfo.attach(`correct-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await form.getByRole("button", { name: "Correct the value" }).click();
      await expect(page.getByText("Corrected by Sami Steward: Counted on site")).toBeVisible();
      await expect(page.getByText("The value is corrected and the finding resolved.")).toBeVisible();
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText(/corrected availableBikeNumber: Counted on site/)).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
