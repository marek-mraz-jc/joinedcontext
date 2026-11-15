import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

// T-3332: the page at a phone, a tablet, a laptop and a wall, light and dark, in Finnish and
// English: the answer on arrival (computed by the WebAssembly module under the static host's CSP),
// the indicators, the chosen one's chart and its odd reading; no sideways scroll, no overlap,
// nothing axe finds.
for (const scheme of ["light", "dark"] as const) {
  for (const lang of ["fi", "en"] as const) {
    for (const size of WIDTHS) {
      test(`${scheme} ${lang} at ${size.width} px: answers on arrival, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { outside, missing, problems } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(`${BASE}?lang=${lang}`);
        const summary = lang === "fi" ? /^4 mittaria, viimeiset 30 päivää: 1 nousee, 1 laskee, 1 pysyy ennallaan\./ : /^4 indicators over the last 30 days: 1 rising, 1 falling, 1 flat\./;
        await expect(page.locator(".app-summary")).toHaveText(summary);
        await expect(page.locator(".app-kpi")).toHaveCount(4);
        await expect(page.locator(".jc-chart-canvas canvas").first()).toBeVisible();
        await expect(page.locator(".app-anomalies li")).toHaveCount(1);
        await testInfo.attach(`${scheme}-${lang}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        // Sideways scroll, overlap, cut-off controls and what axe finds at WCAG 2.1 AA.
        expect(await layoutProblems(page)).toEqual([]);
        expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
      });
    }
  }
}

// SDK-40: the chosen indicator in the SDK's entity panel, at a phone and a laptop, light and dark.
// A public App writes nothing, so the panel links to the indicator in the Portal instead of Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an indicator in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await expect(page.locator(".app-summary")).toHaveText(/^4 indicators /);
      await page.getByRole("button", { name: "Demo counter" }).click();
      await page.getByRole("button", { name: "All details" }).click();
      const panel = page.getByRole("dialog", { name: "Demo counter" });
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

test("a picked indicator and the period go into the address and come back from it", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  await expect(page.locator(".app-summary")).toHaveText(/^4 indicators /);
  await page.getByRole("button", { name: "Demo counter" }).click();
  await page.getByRole("combobox", { name: "Period" }).selectOption("7");
  await expect(page.locator(".app-summary")).toHaveText(/^4 indicators over the last 7 days/);
  const url = new URL(page.url());
  expect(url.searchParams.get("days")).toBe("7");
  expect(url.searchParams.get("kpi")).toMatch(/demo-counter-1$/);
  await page.reload();
  await expect(page.getByRole("button", { name: "Demo counter" })).toHaveAttribute("aria-pressed", "true");
});

test("no indicators says so instead of an empty page", async ({ page }) => {
  await serve(page, [], []);
  await page.goto(`${BASE}?lang=en`);
  await expect(page.getByText("No indicators.")).toBeVisible();
});

// T-3827: a viewer follows an indicator and finds it under Followed; a steward publishes a forecast
// every reader then sees. At a phone and a laptop, light and dark, in English and Swedish.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer follows an indicator, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await expect(page.getByLabel("Your role: Viewer")).toBeVisible();
      await expect(page.getByText("No forecast of this indicator is published.")).toBeVisible();
      await expect(page.getByRole("button", { name: "Publish the forecast" })).toBeDisabled();
      await page.getByRole("button", { name: "Follow", exact: true }).click();
      await expect(page.getByRole("button", { name: "Stop following", exact: true })).toBeVisible();
      await testInfo.attach(`viewer-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "Followed" }).click();
      await expect(page.getByRole("link", { name: "Open: Bikes available in the city bike network" })).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText(/followed an indicator/)).toBeVisible();
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward publishes a forecast, in Swedish, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=sv`);
      await expect(page.getByLabel("Din roll: Förvaltare")).toBeVisible();
      const form = page.getByRole("form", { name: "Publicera prognosen" });
      await form.getByRole("textbox", { name: "Steg framåt (1–48)" }).fill("6");
      await form.getByRole("textbox", { name: "Anteckning" }).fill("Till styrelsen");
      await form.getByRole("button", { name: "Publicera prognosen" }).click();
      await expect(page.getByText("Prognosen är publicerad.")).toBeVisible();
      await expect(page.locator(".app-quote")).toHaveText("Till styrelsen");
      await expect(page.locator(".app-points tbody tr")).toHaveCount(6);
      await testInfo.attach(`steward-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "Historik" }).click();
      await expect(page.getByText(/published a forecast: Till styrelsen/)).toBeVisible();
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
