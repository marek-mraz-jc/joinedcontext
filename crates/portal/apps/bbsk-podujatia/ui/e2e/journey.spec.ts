import { expect, test } from "@playwright/test";
import type { Page, TestInfo } from "@playwright/test";
import { LIVE_BLOCKS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, serve, SLUG } from "./serve";

const ICS = `http://portal.test/api/endpoint/${SLUG}/ical/v1/events.ics`;

// T-4897: a resident opens the region's events, keeps Zvolen's and subscribes to the calendar.
test("open, filter Zvolen, subscribe to the calendar", async ({ page }) => {
  const served = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(BASE);
  await expect(page.getByText("17 podujatí")).toBeVisible();
  await page.getByLabel("Mesto alebo obec").selectOption("Zvolen");
  await expect(page.getByText("2 podujatia")).toBeVisible();
  await page.getByRole("tab", { name: "Zoznam" }).click();
  await expect(page.getByRole("button", { name: /Zvolenské divadelné dni/ })).toBeVisible();
  await expect(page.getByRole("link", { name: /Stránka podujatia Beseda o histórii zámku/ })).toHaveAttribute("href", "https://www.zvolen.sk/podujatia/beseda");
  const region = page.getByRole("textbox", { name: /Celý kraj/ });
  await expect(region).toHaveValue(ICS);
  await expect(page.getByRole("link", { name: "Otvoriť v kalendári" }).first()).toHaveAttribute("href", ICS.replace("http://", "webcal://"));
  await page.getByLabel("Od", { exact: true }).fill("2026-11-01");
  await expect(page.getByRole("textbox", { name: /Vybrané dátumy/ })).toHaveValue(`${ICS}?from=2026-11-01`);
  await expect(page.getByText("Kalendár zatiaľ filtruje len podľa dátumu")).toBeVisible();
  expect(served).toEqual({ outside: [], missing: [], problems: [] });
});

/** One state at one width: laid out without overlap or sideways scroll, and kept as a picture. */
async function shot(page: Page, testInfo: TestInfo, name: string, fullPage = true) {
  expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
  // The entity panel is fixed to the viewport, which a full-page picture draws out of place.
  await page.screenshot({ path: testInfo.outputPath(`${name}.png`), fullPage });
}

for (const width of [1440, 400]) {
  test(`every state at ${width} px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const served = await serve(page);
    await page.goto(BASE);
    await expect(page.getByText("17 podujatí")).toBeVisible();
    await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
    await shot(page, testInfo, `map-${width}`);
    await page.getByRole("tab", { name: "Zoznam" }).click();
    await shot(page, testInfo, `list-${width}`);
    await page.getByRole("tab", { name: "Kalendár" }).click();
    await page.getByRole("button", { name: /30\. októbra: 2 podujatia/ }).click();
    await shot(page, testInfo, `calendar-${width}`);
    await page.getByRole("button", { name: "Nasledujúci mesiac" }).click();
    await shot(page, testInfo, `calendar-november-${width}`);
    await page.getByRole("tab", { name: "Zoznam" }).click();
    await page.getByRole("button", { name: /Beseda o histórii zámku/ }).click();
    await expect(page.getByRole("dialog", { name: "Beseda o histórii zámku" })).toBeVisible();
    await shot(page, testInfo, `detail-${width}`, false);
    await page.keyboard.press("Escape");
    await page.getByLabel("Hľadať").fill("nič také");
    await expect(page.getByText(/Filtrom nevyhovuje/)).toBeVisible();
    await shot(page, testInfo, `empty-filter-${width}`);
    await page.getByLabel("Od", { exact: true }).fill("2026-11-02");
    await page.getByLabel("Do", { exact: true }).fill("2026-11-01");
    await expect(page.getByRole("alert")).toBeVisible();
    await shot(page, testInfo, `range-invalid-${width}`);
    expect(served).toEqual({ outside: [], missing: [], problems: [] });
  });

  test(`no events yet, and a refused read, at ${width} px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, { rows: [] });
    await page.goto(BASE);
    await expect(page.getByText(/Zatiaľ tu nie sú žiadne/)).toBeVisible();
    await shot(page, testInfo, `no-events-${width}`);
    const other = await page.context().newPage();
    await other.setViewportSize({ width, height: 900 });
    await serve(other, { status: 503 });
    await other.goto(BASE);
    await expect(other.getByRole("alert")).toContainText("Podujatia sa nepodarilo načítať");
    await shot(other, testInfo, `error-${width}`);
  });

  test(`in English at ${width} px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, { language: "en" });
    await page.goto(BASE);
    await page.getByRole("tab", { name: "Calendar" }).click();
    await expect(page.getByRole("heading", { name: "Add to my calendar" })).toBeVisible();
    await shot(page, testInfo, `calendar-en-${width}`);
  });
}
