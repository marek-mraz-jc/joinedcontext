import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

for (const scheme of ["light", "dark"] as const) {
  for (const lang of ["fi", "en", "sv"] as const) {
    for (const size of WIDTHS) {
      test(`${scheme} ${lang} at ${size.width} px: answers on arrival, map canvas, tiles, chart, heat table, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { outside, missing, problems } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(`${BASE}?lang=${lang}`);

        // The map canvas is drawn
        await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);

        // The station selector is present
        const combobox = page.getByRole("combobox", { name: lang === "fi" ? "Asema" : "Station", exact: true });
        await expect(combobox).toBeVisible();

        // Availability tiles are visible
        const tiles = page.locator(".jc-tiles .jc-tile");
        await expect(tiles).toHaveCount(3);

        // The chart canvas is drawn
        await expect(page.locator(".jc-chart-canvas")).toHaveCount(1);

        // The profile heat table is visible
        await expect(page.locator(".app-heat-table")).toBeVisible();

        await testInfo.attach(`${scheme}-${lang}-${size.width}.png`, {
          body: await page.screenshot({ fullPage: true }),
          contentType: "image/png",
        });

        expect(await layoutProblems(page)).toEqual([]);
        expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
      });
    }
  }
}

test("changing the station in combobox updates URL hash and tiles", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);

  const combobox = page.getByRole("combobox", { name: "Station", exact: true });
  await expect(combobox).toBeVisible();

  await combobox.selectOption({ label: "Hakaniemi (10 / 20)" });
  expect(page.url()).toContain("id=urn%3Angsi-ld%3ABikeHireDockingStation%3Ahel.fi%3Ahelsinki%3Astation-2");

  await expect(page.getByText("No history available for this station.").first()).toBeVisible();
});

test("switching language toggles between Finnish and English", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=fi`);

  await expect(page.getByRole("heading", { level: 1, name: "Pyörät ja sää" })).toBeVisible();

  await page.getByRole("combobox", { name: "Kieli" }).selectOption("en");
  await expect(page.getByRole("heading", { level: 1, name: "Bikes and the weather" })).toBeVisible();
  expect(page.url()).toContain("lang=en");
});

// T-3355: the station's models the App's server keeps, after a reload too, and a day's training
// data downloaded from the store.
test("the station's kept models, after a reload too, and a day's training data", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  const models = page.getByRole("table", { name: "The station's model over time: kept by the server" });
  await expect(models.getByRole("row")).toHaveCount(3);
  await page.reload();
  await expect(models.getByRole("row").nth(1)).toContainText("+0.42");
  await page.context().route("http://portal.test/store/**", (route) => route.fulfill({ status: 200, contentType: "application/json", body: "{}" }));
  const popup = page.waitForEvent("popup");
  await models.getByRole("button", { name: "Download the training data of 21 Oct 2030" }).click();
  const tab = await popup;
  await tab.waitForLoadState();
  expect(tab.url()).toBe("http://portal.test/store/training/2.json");
  await tab.close();
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

test("empty stations shows message instead of broken UI", async ({ page }) => {
  await serve(page, []);
  await page.goto(`${BASE}?lang=en`);
  await expect(page.getByText("No bike stations readable.")).toBeVisible();
});

// T-3390, SDK-40: the chosen station opened in the SDK's entity panel by its Details button, at a
// phone and a laptop, light and dark. The App is public, so the panel links to the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: the station in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await page.getByRole("button", { name: "Details: Rautatientori" }).click();
      const panel = page.getByRole("dialog", { name: "Rautatientori" });
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

// T-3827: a viewer reads a station's model inputs, every control closed with its reason; a steward
// sets them and finds the change in History. At a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer reads the model inputs, closed with the reason, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await expect(page.getByLabel("Role: Viewer")).toBeVisible();
      const form = page.getByRole("form", { name: "Model inputs" });
      await expect(page.getByTestId("inputs-now")).toHaveText("The model reads the weather of the nearest within 10 km and trains on 7 days. The defaults.");
      await expect(form.getByLabel("Days to train on (1–7)")).toBeDisabled();
      await expect(form.getByLabel("Days to train on (1–7)")).toHaveAttribute("title", "Only a steward changes the model inputs.");
      await expect(form.getByRole("button", { name: "Save inputs" })).toBeDisabled();
      await form.scrollIntoViewIfNeeded();
      await testInfo.attach(`viewer-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward sets the model inputs and finds them in History, in Swedish, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, undefined, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=sv`);
      await expect(page.getByLabel("Roll: Förvaltare")).toBeVisible();
      const form = page.getByRole("form", { name: "Modellens indata" });
      await form.getByLabel("Väderstation").selectOption({ label: "Kaisaniemi" });
      await form.getByLabel("Dagar att träna på (1–7)").fill("3");
      await form.getByLabel("Anteckning").fill("Närmare station");
      await form.getByRole("button", { name: "Spara indata" }).click();
      await expect(page.getByText("Sparat: dagens modell tränas på nytt.")).toBeVisible();
      await expect(page.getByTestId("inputs-now")).toContainText("Modellen läser vädret från Kaisaniemi och tränar på 3 dagar. Satt av Sami Steward");
      await expect(page.getByText(/Vald väderstation: Kaisaniemi/)).toBeVisible();
      await form.scrollIntoViewIfNeeded();
      await testInfo.attach(`steward-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "Historik" }).click();
      await expect(page.getByRole("table", { name: "Ändringar" })).toContainText("Sami Steward");
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
