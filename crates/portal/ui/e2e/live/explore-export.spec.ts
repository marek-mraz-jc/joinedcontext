/**
 * T-3253 — exporting a view on dev: the steward picks a type the endpoint grants and exports the
 * whole view in each format; every file downloads and opens as its format says (the CSV for
 * Excel with its byte-order mark and `;`, the JSON as an array, the GeoJSON as a feature
 * collection when the type has a location). Read only.
 */
import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(300_000);

test("a granted type's view exports to every format and each file opens", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/explore?lang=en");
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Explore data" })).toBeVisible({ timeout: 60_000 });
    await page.getByRole("button", { name: "Helsinki city context" }).click();
    const types = page.locator("#explore-type");
    await expect(types.locator("optgroup option").first()).toBeAttached({ timeout: 60_000 });
    const granted = await types
      .locator("option")
      .evaluateAll((options) =>
        options
          .filter((option) => option.getAttribute("value") && option.parentElement?.tagName !== "OPTGROUP")
          .map((option) => option.getAttribute("value") ?? ""),
      );
    expect(granted.length, "the chosen endpoint grants at least one type").toBeGreaterThan(0);
    await types.selectOption(granted[0]);
    await expect(page.locator("tbody tr").first()).toBeVisible({ timeout: 60_000 });

    for (const format of ["csv-excel", "csv", "json", "geojson"]) {
      await page.getByLabel("Export format").selectOption(format);
      await page.getByRole("button", { name: "Export the view" }).click();
      const link = page.getByRole("link", { name: /^Download / });
      const nothing = page.getByText(/no GeoJSON to export|no rows to export/);
      await expect(link.or(nothing)).toBeVisible({ timeout: 120_000 });
      if (await nothing.isVisible()) {
        // Only a type without a location has no GeoJSON; every other format holds the rows.
        expect(format, "only GeoJSON may have nothing to hold").toBe("geojson");
        continue;
      }
      const [download] = await Promise.all([page.waitForEvent("download"), link.click()]);
      const text = await readFile((await download.path()) ?? "", "utf8");
      if (format === "csv-excel") {
        expect(text.startsWith("﻿id;")).toBe(true);
        expect(text.split("\r\n").length).toBeGreaterThan(2);
      } else if (format === "csv") {
        expect(text.startsWith("id,")).toBe(true);
      } else if (format === "json") {
        expect(Array.isArray(JSON.parse(text))).toBe(true);
      } else {
        expect(JSON.parse(text).type).toBe("FeatureCollection");
      }
    }
  } finally {
    await context.close();
  }
});
