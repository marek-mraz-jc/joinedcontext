import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

// T-3330, UI-84, SDK-12: the comparison at a phone, a tablet, a laptop and a wall, light and dark,
// with the WebAssembly statistics run in their worker under the static host's policy: no sideways
// scroll, no two blocks over each other, nothing axe finds at WCAG 2.1 AA.
for (const scheme of ["light", "dark"] as const) {
  for (const size of WIDTHS) {
    test(`${scheme} at ${size.width} px: the answer is given, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const compare = page.getByRole("region", { name: "Compare" });
      await expect(compare.getByRole("region", { name: "The answer" })).toContainText("falls as wind speed");
      await expect(compare.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      // Each chart draws (a chart may stack a second canvas for its hover layer).
      await expect(compare.locator(".jc-chart-canvas").filter({ has: page.locator("canvas") })).toHaveCount(2);
      await testInfo.attach(`compare-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

test("a longer period and another station stay chosen after a reload", async ({ page }) => {
  await serve(page);
  await page.goto(BASE);
  await page.getByRole("combobox", { name: "Period" }).selectOption("7");
  await page.getByRole("combobox", { name: "Air quality station" }).selectOption("makelankatu");
  await page.reload();
  await expect(page.getByRole("combobox", { name: "Period" })).toHaveValue("7");
  await expect(page.getByRole("combobox", { name: "Air quality station" })).toHaveValue("makelankatu");
});

// T-3375, SDK-40, AP-140: a station opened in the SDK's entity panel at a phone and a laptop, light
// and dark. A public App: the panel reads the station and links to it in the Portal, with no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a station in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: "Details of Kallio 2" }).click();
      const panel = page.getByRole("dialog", { name: "Kallio 2" });
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toHaveAttribute("href", /entityId=urn%3Angsi-ld%3AAirQualityObserved/);
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await panel.getByRole("button", { name: "Close" }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3827: a viewer keeps a place and finds it in their history; a steward marks a station's
// readings faulty and the comparison leaves those hours out. At a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer keeps a place, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await expect(page.getByLabel("Your role: Viewer")).toBeVisible();
      await expect(page.getByText("Only a steward marks readings faulty.")).toBeVisible();
      await page.getByRole("button", { name: "My places" }).click();
      const add = page.getByRole("form", { name: "Add the place" });
      await expect(add.getByRole("textbox", { name: "Name" })).toHaveValue("Kallio 2");
      await add.getByRole("button", { name: "Add the place" }).click();
      await expect(page.getByRole("status")).toHaveText("Place added: Kallio 2.");
      await expect(page.getByRole("link", { name: "Open in the comparison: Kallio 2" })).toBeVisible();
      await testInfo.attach(`places-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText(/kept the place Kallio 2/)).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward marks readings faulty, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await expect(page.getByLabel("Your role: Steward")).toBeVisible();
      const form = page.getByRole("form", { name: "Mark faulty" });
      await form.getByLabel("Reason").fill("sensor cleaned");
      await form.getByRole("button", { name: "Mark faulty" }).click();
      await expect(page.getByText("Marked faulty.")).toBeVisible();
      await expect(page.getByText("6 hours left out as faulty.")).toBeVisible();
      await expect(page.getByRole("button", { name: /^Take the mark back: / })).toBeVisible();
      await testInfo.attach(`faults-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText(/marked faulty: sensor cleaned/)).toBeVisible();
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
