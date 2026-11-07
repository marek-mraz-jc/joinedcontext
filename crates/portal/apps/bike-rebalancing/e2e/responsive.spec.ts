import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

// T-3328, UI-84, SDK-12: the plan at a phone, a tablet, a laptop and a wall, light and dark, with
// the WebAssembly planner run in its worker under the static host's policy: no sideways scroll, no
// two blocks over each other, nothing axe finds at WCAG 2.1 AA.
for (const scheme of ["light", "dark"] as const) {
  for (const size of WIDTHS) {
    test(`${scheme} at ${size.width} px: the route is planned, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(BASE);
      const plan = page.getByRole("region", { name: "Rebalancing" });
      await expect(plan.getByRole("list", { name: "Route" }).getByRole("listitem").first()).toBeVisible();
      await expect(plan.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      await expect(plan.locator(".jc-chart-canvas canvas")).toHaveCount(1);
      await testInfo.attach(`plan-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

test("a station left out stays out after a reload, by the address", async ({ page }) => {
  await serve(page);
  await page.goto(BASE);
  const route = page.getByRole("list", { name: "Route" });
  const first = route.getByRole("listitem").first();
  const name = (await first.locator("strong").textContent()) ?? "";
  await first.getByRole("button", { name: `Leave out of the route: ${name}` }).click();
  await expect(route.getByText(name, { exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole("list", { name: "Route" }).getByRole("listitem").first()).toBeVisible();
  await expect(page.getByRole("list", { name: "Route" }).getByText(name, { exact: true })).toHaveCount(0);
});

// T-3388, SDK-40: a stop of the route opened in the SDK's entity panel by its name, at a phone and a
// laptop, light and dark. The operator may only read, so the panel links to the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a station in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      const stop = page.getByRole("list", { name: "Route" }).getByRole("listitem").first();
      const name = (await stop.locator("strong").textContent()) ?? "";
      await stop.getByRole("button", { name, exact: true }).click();
      const panel = page.getByRole("dialog", { name });
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

// T-3826: the levels and the history as a viewer reads them and a steward changes them, at a phone
// and a laptop, light and dark: the viewer's form closed with its reason, the steward's save shown
// and then in the history.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    for (const [who, me] of [
      ["viewer", VIEWER],
      ["steward", STEWARD],
    ] as const) {
      test(`${scheme} at ${width} px: the levels and the history as a ${who}, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const served = await serve(page, me);
        await page.setViewportSize({ width, height: 900 });
        await page.goto(BASE);
        await expect(page.getByLabel(`Your role: ${who === "steward" ? "Steward" : "Viewer"}`)).toBeVisible();
        await page.getByRole("button", { name: "Levels" }).click();
        const low = page.getByRole("spinbutton", { name: "About to run empty below (%)" });
        await expect(low).toHaveValue("15");
        if (who === "viewer") {
          await expect(low).toBeDisabled();
          await expect(page.getByText("Only a steward changes the levels.")).toBeVisible();
        } else {
          await expect(low).toBeEnabled();
          await low.fill("20");
          await page.getByRole("button", { name: "Save the levels" }).click();
          await expect(page.getByRole("status")).toHaveText("The levels are saved. The next plans use them.");
        }
        await testInfo.attach(`levels-${who}-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        expect(await layoutProblems(page)).toEqual([]);
        await page.getByRole("button", { name: "History" }).click();
        await expect(page.getByText(who === "steward" ? "changed: levels changed" : "No change yet.")).toBeVisible();
        await testInfo.attach(`history-${who}-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        expect(await layoutProblems(page)).toEqual([]);
        expect(served).toEqual({ outside: [], missing: [], problems: [] });
      });
    }
  }
}
