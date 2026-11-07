import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { NETWORK_ROWS } from "../src/fixtures/network";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

// T-3331: the page at a phone, a tablet, a laptop and a wall, light and dark, in Finnish and
// English: the answer on arrival (computed by the WebAssembly module under the static host's CSP),
// the map, the legend and the stops reached; no sideways scroll, no overlap, nothing axe finds.
for (const scheme of ["light", "dark"] as const) {
  for (const lang of ["fi", "en"] as const) {
    for (const size of WIDTHS) {
      test(`${scheme} ${lang} at ${size.width} px: answers on arrival, no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { outside, missing, problems } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(`${BASE}?lang=${lang}`);
        const summary = lang === "fi" ? /^Saavutettava alue tästä pisteestä: 10 min .* Pysäkkejä 30 minuutissa: 4\/4\.$/ : /^Reachable from this point: 10 min .* Stops within 30 minutes: 4 of 4\.$/;
        await expect(page.locator(".app-summary")).toHaveText(summary);
        await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
        await expect(page.locator(".app-places li")).toHaveCount(4);
        await testInfo.attach(`${scheme}-${lang}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
        // Sideways scroll, overlap, cut-off controls and what axe finds at WCAG 2.1 AA.
        expect(await layoutProblems(page)).toEqual([]);
        expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
      });
    }
  }
}

test("a click on the map starts from there, and the address keeps the point", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  await expect(page.locator(".app-summary")).toHaveText(/Stops within 30 minutes: 4 of 4\./);
  const map = page.getByTestId("jc-map");
  const box = await map.boundingBox();
  if (!box) throw new Error("the map has no box");
  // Some 20 km east of the centre, past every stop (the notice of a missing basemap sits top left).
  await map.click({ position: { x: box.width - 60, y: box.height / 2 } });
  await expect(page.locator(".app-summary")).not.toHaveText(/Stops within 30 minutes: 4 of 4\./);
  expect(new URL(page.url()).searchParams.get("at")).toMatch(/^2[45]\.\d+,60\.\d+$/);
  await page.getByRole("button", { name: "Back to Rautatientori" }).click();
  await expect(page.locator(".app-summary")).toHaveText(/Stops within 30 minutes: 4 of 4\./);
});

test("no vehicle history answers with walking alone", async ({ page }) => {
  await serve(page, []);
  await page.goto(`${BASE}?lang=en`);
  await expect(page.getByText("No vehicle history: the area is walking distance only.")).toBeVisible();
  await expect(page.locator(".app-summary")).toHaveText(/Stops within 30 minutes: 0 of 0\./);
});

test("with HSL's stops and lines it rides the network, names the stops and asks for no vehicle history", async ({ page }) => {
  const { outside, missing, problems } = await serve(page, [], NETWORK_ROWS);
  const asked: string[] = [];
  page.on("request", (request) => asked.push(request.url()));
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  await expect(page.locator(".app-summary")).toHaveText(/Stops within 30 minutes: 4 of 4\./);
  await expect(page.getByText(/^Stops and lines from HSL's registers \(4 stops, 2 line variants\)\./)).toBeVisible();
  await expect(page.getByRole("button", { name: "Kaisaniemi (H0012): lines 550, M1: start from here" })).toBeVisible();
  expect(asked.some((url) => url.includes("/temporal/"))).toBe(false);
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

// SDK-40, AP-140: a reached HSL stop opens in the entity panel, light and dark: a Portal link, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const size of WIDTHS) {
    test(`${scheme} at ${size.width} px: a reached stop opens in the entity panel, read only, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page, [], NETWORK_ROWS);
      await page.setViewportSize(size);
      await page.goto(`${BASE}?lang=en`);
      await page.getByRole("button", { name: "Kaisaniemi (H0012): lines 550, M1: details" }).click();
      const panel = page.getByRole("dialog");
      await expect(panel).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toHaveAttribute("href", /^https:\/\/portal\.test\/projects\/helsinki\//);
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await panel.getByRole("button", { name: "Close" }).click();
      await expect(panel).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3826: a viewer keeps an origin and compares it with the published one; a steward publishes
// one for everyone; at a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer keeps an origin and compares, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, [], NETWORK_ROWS, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await expect(page.getByLabel("Your role: Viewer")).toBeVisible();
      await page.getByRole("button", { name: "Kaisaniemi (H0012): lines 550, M1: start from here" }).click();
      await expect(page.getByRole("textbox", { name: "Name" })).toHaveValue("Kaisaniemi");
      await page.getByRole("button", { name: "Keep", exact: true }).click();
      await expect(page.getByText("Kept: Kaisaniemi. Compare it on the Origins page.")).toBeVisible();
      await page.getByRole("button", { name: "Origins" }).click();
      const table = page.getByRole("table", { name: "Origins side by side" });
      await expect(table.getByRole("rowheader")).toHaveText(["Kaisaniemi", "Pasila", "Kamppi"]);
      await expect(table.getByRole("button", { name: "Publish: Kaisaniemi" })).toBeDisabled();
      await testInfo.attach(`origins-viewer-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "History" }).click();
      await expect(page.getByText("added: kept the origin Kaisaniemi")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward publishes an origin, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, [], NETWORK_ROWS, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en#origins`);
      await expect(page.getByLabel("Your role: Steward")).toBeVisible();
      const table = page.getByRole("table", { name: "Origins side by side" });
      await table.getByRole("button", { name: "Publish: Kamppi" }).click();
      await expect(page.getByText("Published for everyone: Kamppi.")).toBeVisible();
      await expect(table.getByRole("button", { name: "Withdraw: Kamppi" })).toBeVisible();
      await testInfo.attach(`origins-steward-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
