// T-5233: the catalogue map's count line, its default NGSI-LD to map mapping and the reader's Mapa
// panel kept in the address. The Endpoint is the routed mock of explore-hosts.mjs (data set
// `stations`: 12 stations, 8 with a location, 10 with an observation space). Shots land in
// test-results/explore/ for the UX read in the task.
import { test, expect } from "@playwright/test";
import path from "node:path";
import { shots, CATALOGUE, serve } from "./explore-hosts.mjs";

const frame = (page) => page.frameLocator("#explore");
const inner = (page) => page.frame({ url: /explore\.html/ });
const seeMap = (f) => f.locator("#map").scrollIntoViewIfNeeded({ timeout: 20000 });
const open = async (page, { lang = "sk", theme, width = 1440, extra = "" } = {}) => {
  await page.setViewportSize({ width, height: 900 });
  await page.goto(`https://${CATALOGUE}/dataset/stations?lang=${lang}${theme ? "&theme=" + theme : ""}${extra ? "&extra=" + encodeURIComponent(extra) : ""}`);
  // The map loads once it is in view (T-5794): the reader scrolls to it.
  await seeMap(frame(page));
};
const shot = (page, name) => page.screenshot({ path: path.join(shots, `${name}.png`), fullPage: true });
const points = (page) => inner(page).evaluate(() => (self.jcExploreMap && self.jcExploreMap.getLayer("jc-point") ? self.jcExploreMap.queryRenderedFeatures({ layers: ["jc-point"] }).map((f) => f.geometry.coordinates[0]) : []));
const hash = (page) => inner(page).evaluate(() => location.hash);

test("the count line matches the table, follows a filter and a pan", async ({ page, context }) => {
  await serve(context);
  await open(page);
  const f = frame(page);
  await expect(f.getByRole("grid").locator("tbody tr")).toHaveCount(12, { timeout: 20000 });
  // 12 in the table, 4 of them without a location: all 8 located ones are in the first view.
  await expect(f.locator("#map-count")).toHaveText("Na mape 8 z 12 · 4 bez polohy", { timeout: 20000 });
  await expect(f.locator("#map-count")).toHaveAttribute("aria-live", "polite");
  await inner(page).evaluate(() => self.jcExploreMap.jumpTo({ center: [18.7, 49.2], zoom: 14 }));
  await expect(f.locator("#map-count")).toHaveText("Na mape 1 z 12 · 4 bez polohy");
  await open(page, { extra: "&q=" + encodeURIComponent("open==true") });
  await expect(f.locator("#map-count")).toHaveText("Na mape 4 z 6 · 2 bez polohy", { timeout: 20000 });
  await expect(f.getByRole("grid").locator("tbody tr")).toHaveCount(6);
});

test("the Mapa panel moves the markers, colours them, round-trips the address, resets and refuses an unknown slot", async ({ page, context }) => {
  await serve(context);
  await open(page);
  const f = frame(page);
  await expect(f.locator("#map-count")).toHaveText(/z 12/, { timeout: 20000 });
  await expect.poll(() => points(page), { timeout: 20000 }).toHaveLength(8);
  expect(Math.max(...(await points(page)))).toBeLessThan(19.2);
  await expect(f.locator("#map-panel")).toBeHidden();
  await f.getByRole("button", { name: "Nastaviť mapu" }).click();
  await expect(f.getByRole("button", { name: "Nastaviť mapu" })).toHaveAttribute("aria-expanded", "true");
  await expect(f.getByLabel("Poloha z", { exact: true })).toHaveValue("location");
  await f.getByLabel("Poloha z", { exact: true }).selectOption({ label: "Priestor pozorovania" });
  await expect(f.locator("#map-count")).toHaveText(/z 12 · 2 bez polohy$/);
  await expect.poll(async () => Math.min(...(await points(page))), { timeout: 20000 }).toBeGreaterThan(19.4);
  await expect(f.getByRole("button", { name: /^Odstrániť Poloha z: Priestor pozorovania/ })).toBeVisible();
  await f.getByLabel("Farba podľa", { exact: true }).selectOption({ label: "Otvorená" });
  await expect(f.locator("#map-legend")).toContainText("Áno");
  await expect(f.locator("#map-legend")).toContainText("Nie");
  await f.getByRole("checkbox", { name: "Zoskupovať body" }).uncheck();
  const shared = await hash(page);
  expect(shared).toContain("&geo=observationSpace");
  expect(shared).toContain("&color=open");
  expect(shared).toContain("&cluster=0");
  for (const width of [1440, 400]) {
    for (const theme of ["light", "dark"]) {
      // Dark as a visitor's system sets it: the Portal bundle's own tokens follow only that (chyby.md).
      await page.emulateMedia({ colorScheme: theme });
      await open(page, { width, extra: shared.replace(/^#[a-z0-9]+&inline&lang=sk/, "") });
      await expect(f.locator("#map-legend")).toContainText("Áno", { timeout: 20000 });
      await expect(f.locator("#map-chips button")).toHaveCount(3);
      await page.waitForTimeout(400);
      await shot(page, `map-panel-shared-${width}-${theme}`);
      if (width === 1440 && theme === "light") {
        await f.getByRole("button", { name: "Nastaviť mapu" }).click();
        await expect(f.getByLabel("Poloha z", { exact: true })).toHaveValue("observationSpace");
        await expect(f.getByRole("checkbox", { name: "Zoskupovať body" })).not.toBeChecked();
        await shot(page, "map-panel-open-1440-light");
      }
    }
  }
  await page.emulateMedia({ colorScheme: "light" });
  await f.getByRole("button", { name: "Obnoviť predvolené" }).click();
  await expect(f.locator("#map-chips button")).toHaveCount(0);
  await expect(f.locator("#map-legend")).toBeHidden();
  expect(await hash(page)).not.toMatch(/&(geo|color|cluster)=/);
  await expect.poll(async () => Math.max(...(await points(page))), { timeout: 20000 }).toBeLessThan(19.2);

  // A hand-edited address naming a slot the model lacks: the default, with a short note.
  await open(page, { extra: "&geo=secretArea&color=nothing" });
  await expect(f.locator("#map-fallback")).toContainText("Poloha z, Farba podľa", { timeout: 20000 });
  await expect(f.locator("#map-count")).toHaveText(/z 12 · 4 bez polohy$/);
  expect(await hash(page)).not.toMatch(/secretArea|nothing/);
  await shot(page, "map-panel-fallback-1440-light");
  await open(page, { width: 400, lang: "en" });
  await expect(f.locator("#map-count")).toHaveText("On the map 8 of 12 · 4 without a location", { timeout: 20000 });
  await shot(page, "map-count-400-en");
});

// T-5239: a publisher set the colour field, the popup and clustering in the Portal (widget.json); the
// widget opens with that legend, the reader's panel starts from it and "Obnoviť predvolené" returns there.
test("the publisher's map defaults start the map, and the reader's reset returns to them", async ({ page, context }) => {
  await serve(context);
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`https://${CATALOGUE}/dataset/publishedmap?lang=sk`);
    const f = frame(page);
    await seeMap(f);
    await expect(f.locator("#map-count")).toHaveText(/z 12/, { timeout: 20000 });
    await expect(f.locator("#map-legend")).toContainText("Áno", { timeout: 20000 });
    await expect(f.locator("#map-legend")).toContainText("Nie");
    // The publisher's default is no reader's choice: no chip, nothing in the address.
    await expect(f.locator("#map-chips button")).toHaveCount(0);
    expect(await hash(page)).not.toMatch(/&(geo|color|pop|cluster)=/);
    await page.waitForTimeout(400);
    await shot(page, `map-publisher-defaults-${width}-light`);
  }
  const f = frame(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await f.getByRole("button", { name: "Nastaviť mapu" }).click();
  await expect(f.getByLabel("Farba podľa", { exact: true })).toHaveValue("open");
  await expect(f.getByRole("checkbox", { name: "Zoskupovať body" })).not.toBeChecked();
  await shot(page, "map-publisher-panel-1440-light");
  await f.getByLabel("Farba podľa", { exact: true }).selectOption("type");
  await expect(f.locator("#map-legend")).toBeHidden();
  expect(await hash(page)).toContain("&color=type");
  await f.getByRole("button", { name: "Obnoviť predvolené" }).click();
  await expect(f.locator("#map-legend")).toContainText("Áno");
  await expect(f.getByLabel("Farba podľa", { exact: true })).toHaveValue("open");
  expect(await hash(page)).not.toMatch(/&color=/);
});
