/**
 * T-6454, journey 4: the public catalogue as a visitor. Search, open a dataset, read its NGSI-LD
 * resource, see its data and its map.
 */
import { expect, test } from "@playwright/test";
import { SIZES, clipOfVisitor } from "./clips";

test.setTimeout(240_000);

for (const size of SIZES) {
  test(`Catalogue at ${size.width}`, async ({ browser }) => {
    const clip = await clipOfVisitor(browser, "catalogue", size, "/catalogue?lang=en");
    const { page } = clip;
    try {
      await clip.step("The public catalogue: every dataset, no sign-in", async () => {
        await expect(page.getByRole("heading", { level: 1, name: "Catalogue" })).toBeVisible({ timeout: 60_000 });
      });
      const results = page.getByRole("region", { name: "Datasets" });
      await clip.step("Search for air quality", async () => {
        await page.getByRole("searchbox").first().fill("air quality");
        await page.keyboard.press("Enter");
        await expect(page).toHaveURL(/air/, { timeout: 30_000 });
        await expect(results.getByRole("heading", { level: 2 }).first()).toBeVisible({ timeout: 30_000 });
      });
      await clip.step("Open a dataset: what it is, who publishes it, how to use it", async () => {
        await results.locator('a[href^="/catalogue/air-quality"]').first().click();
        await expect(page).toHaveURL(/\/catalogue\/air-quality/);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 30_000 });
      });
      await clip.step("Its resources, the NGSI-LD address among them", async () => {
        const resources = page.getByRole("heading", { name: "Resources" });
        await resources.scrollIntoViewIfNeeded();
        await expect(page.getByRole("table", { name: "Resources" }).getByText(/NGSI-LD/).first()).toBeVisible({ timeout: 30_000 });
      });
      await clip.step("Its data, the same rows the API serves", async () => {
        await page.getByRole("heading", { name: "Data", exact: true }).scrollIntoViewIfNeeded();
      });
      await clip.step("Its map", async () => {
        const map = page.locator("canvas.maplibregl-canvas").first();
        await map.scrollIntoViewIfNeeded();
        await expect(map).toBeVisible({ timeout: 30_000 });
      });
    } finally {
      await clip.finish();
    }
  });
}
