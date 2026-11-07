import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

// T-3329, UI-84, SDK-12: the day at a phone, a tablet, a laptop and a wall, light and dark, with the
// WebAssembly planner run in its worker under the static host's policy: no sideways scroll, no two
// blocks over each other, nothing axe finds at WCAG 2.1 AA.
for (const scheme of ["light", "dark"] as const) {
  for (const size of WIDTHS) {
    test(`${scheme} at ${size.width} px: a day is suggested, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const day = page.getByRole("region", { name: "My day" });
      await expect(day.getByRole("list", { name: "The plan" }).getByRole("listitem").first()).toBeVisible();
      await expect(day.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      await expect(day.locator(".jc-chart-canvas canvas")).toHaveCount(1);
      await testInfo.attach(`day-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

test("picked events are planned in order and stay picked after a reload", async ({ page }) => {
  await serve(page);
  await page.goto(BASE);
  const list = page.getByRole("list", { name: "Events of the day" });
  await list.getByRole("checkbox", { name: /Jazz at Stoa/ }).check();
  await list.getByRole("checkbox", { name: /Workshop for Families/ }).check();
  const plan = page.getByRole("list", { name: "The plan" });
  await expect(plan.getByRole("listitem")).toHaveCount(2);
  await expect(plan.getByRole("listitem").first()).toContainText("Workshop for Families");
  await page.reload();
  await expect(page.getByRole("list", { name: "The plan" }).getByRole("listitem").last()).toContainText("Jazz at Stoa");
});

// SDK-40, T-3393: an event of the plan opens in the shell's entity panel, read through the app's
// endpoint, at a phone and a laptop, light and dark; a public App writes nothing, so the panel links
// to the Portal. Finnish speaks in the panel too.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an event in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("list", { name: "The plan" }).getByRole("button", { name: "Workshop for Families" }).click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("Siltakatu 11, Helsinki")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

test("switches to Finnish, and the panel speaks it", async ({ page }) => {
  await serve(page);
  await page.goto(BASE);
  await page.getByRole("combobox", { name: "Language" }).selectOption("fi");
  await expect(page.getByRole("heading", { level: 1, name: "Tapahtumapäiväni" })).toBeVisible();
  // The page's words switch; the events keep the language the Portal serves them in.
  await page.getByRole("list", { name: "Suunnitelma" }).getByRole("button", { name: "Workshop for Families" }).click();
  await expect(page.getByRole("dialog").getByRole("link", { name: "Avaa portaalissa" })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { level: 1, name: "Tapahtumapäiväni" })).toBeVisible();
});

// T-3826: a steward features an event and notes it; a viewer finds it, keeps their days and reads
// their history, at a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a steward features an event, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await expect(page.getByLabel("Your role: Steward")).toBeVisible();
      await page.getByRole("list", { name: "Events of the day" }).getByRole("button", { name: "Feature: Organ concert" }).click();
      const featured = page.getByRole("list", { name: "Featured events" });
      await expect(featured.getByText("Organ concert", { exact: true })).toBeVisible();
      await featured.getByRole("textbox", { name: "Note" }).fill("Free entry, come early");
      await featured.getByRole("button", { name: "Save the note" }).click();
      await expect(page.getByText("The note is saved.")).toBeVisible();
      await testInfo.attach(`featured-steward-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText("changed: changed the note")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a viewer's days and history, nothing to feature, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await expect(page.getByLabel("Your role: Viewer")).toBeVisible();
      await expect(page.getByText("A steward curates the featured events.")).toBeVisible();
      await expect(page.getByRole("button", { name: /^Feature:/ })).toHaveCount(0);
      await page.getByRole("button", { name: "My days" }).click();
      await expect(page.getByRole("link", { name: /^Open: .*2030 · 2 events/ })).toBeVisible();
      await testInfo.attach(`days-viewer-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText("No change yet.")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
