import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, serve } from "./serve";

// T-3333: the page at a phone, a tablet, a laptop and a wall, light and dark, in Finnish, English and Swedish: the answer on
// arrival (computed by the WebAssembly module under the static host's CSP), the map, the hours of
// the week, the repeat places; no sideways scroll, no overlap, nothing axe finds.
for (const scheme of ["light", "dark"] as const) {
for (const lang of ["fi", "en", "sv"] as const) {
  for (const size of WIDTHS) {
    test(`${scheme} ${lang} at ${size.width} px: answers on arrival, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(`${BASE}?lang=${lang}`);
      const summary = { fi: /^8 tiedotetta 7\.10\.2030–21\.10\.2030\./, en: /^8 alerts from 7 Oct 2030 to 21 Oct 2030\./, sv: /^8 meddelanden från / }[lang];
      await expect(page.locator(".app-summary")).toHaveText(summary);
      await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
      // One chart; ECharts draws a heat map on more than one canvas layer.
      await expect(page.locator(".jc-chart-canvas")).toHaveCount(1);
      await expect(page.locator(".jc-chart-canvas canvas").first()).toBeVisible();
      await expect(page.locator(".app-places li")).toHaveCount(1);
      await testInfo.attach(`${scheme}-${lang}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      // Sideways scroll, overlap, cut-off controls and what axe finds at WCAG 2.1 AA.
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
}

test("a click on an hour of the week keeps only that hour, and the address says so", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en&day=0&hour=8`);
  await expect(page.locator(".app-summary")).toHaveText(/^4 alerts /);
  await page.getByRole("button", { name: "Show every hour, not only Monday 08:00–09:00" }).click();
  await expect(page.locator(".app-summary")).toHaveText(/^8 alerts /);
  expect(new URL(page.url()).searchParams.get("day")).toBeNull();
});

// T-3351: the view saved on the App's server with a picture of the map, and shown again.
test("saves the view as a report with its map picture, and shows it again from the list", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en&kind=roadWorks`);
  await expect(page.locator(".app-summary")).toHaveText(/ alerts /);
  await expect(page.getByRole("table", { name: "Repeat places week by week" }).getByRole("row")).toHaveCount(3);
  await page.getByLabel("Report name").fill("Road works");
  const picture = page.waitForRequest((request) => request.method() === "PUT" && request.url().startsWith("http://portal.test/store/"));
  await page.getByRole("button", { name: "Save this view" }).click();
  await expect(page.getByRole("status")).toHaveText("Report saved: Road works.");
  expect((await picture).headers()["content-type"]).toBe("image/png");
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get("kind")).toBeNull();
  await page.reload();
  await page.getByRole("button", { name: "Show the report Road works" }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get("kind")).toBe("roadWorks");
  await expect(page.getByRole("button", { name: "Open the map picture of Road works" })).toBeVisible();
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

test("no alerts says so instead of an empty map", async ({ page }) => {
  await serve(page, []);
  await page.goto(`${BASE}?lang=en`);
  await expect(page.getByText("No alerts.")).toBeVisible();
});

// T-3377, SDK-40, AP-140: a repeat place's alert opened in the SDK's entity panel at a phone and a
// laptop, light and dark. A public App: the panel reads it and links to it in the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a repeat place's alert in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await page.getByRole("button", { name: "Details of Mannerheimintie, Helsinki. Tietyö." }).click();
      const panel = page.getByRole("dialog", { name: "Mannerheimintie, Helsinki. Tietyö." });
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toHaveAttribute("href", /entityId=urn%3Angsi-ld%3AAlert/);
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await panel.getByRole("button", { name: "Close" }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3828: a viewer keeps the filters on screen and applies them again; a steward acknowledges an
// alert in Swedish and takes it back; both see the history, at a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer keeps a filter and applies it again; acknowledging stays a steward's`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en&kind=roadWorks`);
      await expect(page.getByLabel("Role: Viewer")).toBeVisible();
      await expect(page.locator(".app-summary")).toHaveText(/ alerts /);
      await page.getByLabel("Name of the filter").fill("Road works");
      await page.getByRole("button", { name: "Keep these filters" }).click();
      await expect(page.getByText("Kept: Road works")).toBeVisible();
      await page.getByRole("button", { name: "Clear filters" }).click();
      await expect.poll(() => new URL(page.url()).searchParams.get("kind")).toBeNull();
      await page.getByRole("button", { name: "Apply Road works" }).click();
      await expect.poll(() => new URL(page.url()).searchParams.get("kind")).toBe("roadWorks");
      const acknowledge = page.getByRole("button", { name: /^Acknowledge Kehä I/ });
      await expect(acknowledge).toBeDisabled();
      await expect(acknowledge).toHaveAttribute("title", "Only a steward acknowledges alerts.");
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "Changes" }).first().click();
      await expect(page.getByText("kept the filter Road works")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward acknowledges an alert in Swedish and takes it back`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page, undefined, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=sv`);
      await expect(page.getByLabel("Roll: Förvaltare")).toBeVisible();
      await page.getByLabel(/^Kommentar om Kehä I/).fill("Polisen informerad");
      await page.getByRole("button", { name: /^Kvittera Kehä I/ }).click();
      await expect(page.getByText(/^Kvitterat: Polisen informerad \(Sara Steward, /)).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: /^Ångra kvitteringen av Kehä I/ }).click();
      await expect(page.getByRole("button", { name: /^Kvittera Kehä I/ })).toBeVisible();
      await page.getByRole("button", { name: "Ändringar" }).first().click();
      await expect(page.getByText("Allas ändringar, nyaste först.")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
