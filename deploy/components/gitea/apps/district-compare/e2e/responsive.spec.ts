import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, VIEWER, serve } from "./serve";

for (const scheme of ["light", "dark"] as const) {
  for (const lang of ["fi", "en", "sv"] as const) {
    for (const size of WIDTHS) {
      test(`${scheme} ${lang} at ${size.width} px: answers on arrival, table with two districts, map canvas, axe clean`, async ({ page }, testInfo) => {
        await page.emulateMedia({ colorScheme: scheme });
        const { outside, missing, problems } = await serve(page);
        await page.setViewportSize(size);
        await page.goto(`${BASE}?lang=${lang}`);

        // The map canvas is drawn
        await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);

        // The ranked list has items
        const rankedList = page.locator(".app-ranked-list");
        await expect(rankedList).toBeVisible();
        await expect(rankedList.locator("li")).toHaveCount(3);

        // The table has two district columns (plus 1 measure column = 3 headers)
        await expect(page.getByRole("table", { name: { en: "Selected districts compared", fi: "Valitut kaupunginosat rinnakkain", sv: "Valda stadsdelar sida vid sida" }[lang] }).locator("thead th")).toHaveCount(3);

        // The chart canvas is drawn
        await expect(page.locator(".jc-chart-canvas")).toHaveCount(1);

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

test("toggling a district in the list adds it to the comparison table and updates URL hash", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);

  // Initially 2 districts (Kamppi and Kallio)
  await expect(page.getByRole("table", { name: "Selected districts compared" }).locator("thead th")).toHaveCount(3);

  // Check Töölö
  await page.getByRole("checkbox", { name: "Töölö" }).check();

  // Now 3 districts (Kamppi, Kallio, Töölö) -> 4 header columns
  await expect(page.getByRole("table", { name: "Selected districts compared" }).locator("thead th")).toHaveCount(4);
  // The address names exactly these three; their order follows the selection, not the code.
  const picked = new URLSearchParams(new URL(page.url()).hash.split("?")[1]).get("d")?.split(",") ?? [];
  expect(picked.sort()).toEqual(["101", "102", "103"]);
});

test("changing the measure updates table rows and chart", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);

  // The data is still arriving when the measure changes: the address must end on the person's choice.
  await page.getByLabel("Measure").selectOption("bikes");
  await expect(page).toHaveURL(/m=bikes/);
  await expect(page.getByRole("table", { name: "Selected districts compared" })).toBeVisible();
  await expect(page).toHaveURL(/m=bikes/);
  await expect(page.locator("figcaption")).toHaveText("Selected districts: City-bike stations");
});

// T-3353: the selected districts' days as the App's server kept them, after a reload too, and the
// boundaries with their licence downloaded from the store.
test("the selected districts' kept days, after a reload too, and the boundaries as a file", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en`);
  const history = page.getByRole("table", { name: "History of the selected districts: Events" });
  await expect(history.getByRole("row")).toHaveCount(3);
  await page.reload();
  await expect(history.getByRole("row").nth(1)).toContainText("21 Oct 2030");
  await page.context().route("http://portal.test/store/**", (route) => route.fulfill({ status: 200, contentType: "text/plain", body: "CC BY 4.0" }));
  const popup = page.waitForEvent("popup");
  await page.getByRole("button", { name: "Download the licence of the boundaries" }).click();
  const tab = await popup;
  await tab.waitForLoadState();
  expect(tab.url()).toBe("http://portal.test/store/boundaries/LICENCE.txt");
  await tab.close();
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

test("empty districts shows message instead of broken UI", async ({ page }) => {
  await serve(page, []);
  await page.goto(`${BASE}?lang=en`);
  await expect(page.getByText("No district boundaries readable.")).toBeVisible();
});

// T-3392, SDK-40: a compared district opened in the SDK's entity panel by its Details button, at a
// phone and a laptop, light and dark. The App is public, so the panel links to the Portal, no Edit.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a district in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await page.getByRole("button", { name: /^Details: / }).first().click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByText("AdministrativeArea")).toBeVisible();
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3828: a viewer keeps the comparison on screen and gets its link to share; a steward pins one
// for everyone and finds it in Changes. At a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer keeps a comparison and gets its link, pinning closed with the reason, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, VIEWER);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en`);
      await expect(page.getByLabel("Role: Viewer")).toBeVisible();
      await expect(page.getByRole("button", { name: "Pin this comparison for everyone" })).toBeDisabled();
      await expect(page.getByRole("button", { name: "Pin this comparison for everyone" })).toHaveAttribute("title", "Only a steward pins a comparison for everyone.");
      await page.getByLabel("Name of the comparison").fill("My two");
      await page.getByRole("button", { name: "Keep this comparison" }).click();
      await expect(page.getByText("Kept: My two")).toBeVisible();
      const link = page.getByRole("link", { name: "Link to share: My two" });
      await expect(link).toHaveAttribute("href", new RegExp(`\\?comparison=${"1".padStart(64, "0")}&lang=en$`));
      await link.scrollIntoViewIfNeeded();
      await testInfo.attach(`viewer-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward pins a comparison for everyone and finds it in Changes, in Swedish, axe clean`, async ({ page }, testInfo) => {
      await page.emulateMedia({ colorScheme: scheme });
      const served = await serve(page, undefined, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=sv`);
      await expect(page.getByLabel("Roll: Förvaltare")).toBeVisible();
      await page.getByLabel("Jämförelsens namn").fill("För alla");
      await page.getByRole("button", { name: "Fäst den här jämförelsen för alla" }).click();
      await expect(page.getByText("Fäst: För alla")).toBeVisible();
      await expect(page.getByTestId("pinned")).toContainText("För alla, fäst av Sami Steward.");
      await page.getByTestId("pinned").scrollIntoViewIfNeeded();
      await testInfo.attach(`steward-${scheme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: "Ändringar" }).click();
      await expect(page.getByRole("table", { name: "Ändringar" })).toContainText("pinned För alla for everyone");
      expect(await layoutProblems(page)).toEqual([]);
      expect(served).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
