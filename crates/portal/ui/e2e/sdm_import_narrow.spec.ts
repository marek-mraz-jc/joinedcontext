// T-3251: the picker's two columns at phone width; the source's long lines used to widen the page.
import { expect, test } from "@playwright/test";
const PROJECT = "banskabystrica";
const IDENTITY = { subject: "s", username: "jana", name: "Jana", email: "jana@bb.sk", roles: ["portal-editor"] };
const LINKML = "id: https://github.com/smart-data-models/dataModel.Environment/AirQualityObserved\nname: AirQualityObserved\nannotations:\n  spec.source.repository: https://github.com/smart-data-models/dataModel.Environment\n  spec.source.path: AirQualityObserved\n  spec.source.commit: 9f1c2b7d4e6a8c0b2d4f6a8c0e2b4d6f8a0c2e4b\nclasses:\n  AirQualityObserved:\n    slots: [pm10, pm25, dateObserved]\nslots:\n  pm10: { range: float }\n  pm25: { range: float }\n  dateObserved: { range: datetime, required: true }\n";
test("the Smart Data Models picker fits a 375 px screen without scrolling sideways (T-3251)", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 1800 });
  await page.route("**/api/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p.endsWith("/auth/me")) return json(IDENTITY);
    if (p.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (p.endsWith("/sdm-catalog")) return json({ subjects: [{ name: "dataModel.Environment", title: "Environment", models: [{ id: "dataModel.Environment/AirQualityObserved", name: "AirQualityObserved", description: "Air quality observed at a station", attributes: ["pm10"] }] }] });
    if (p.endsWith("/import-sdm")) return json({ linkml: LINKML });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  await page.goto(`/projects/${PROJECT}/models?new=sdm&lang=en`);
  // New data model is one window for the name, then Add type offers the import (T-6344).
  const naming = page.getByRole("dialog", { name: "New data model" });
  await naming.getByRole("textbox", { name: "Name", exact: true }).fill("Air");
  await naming.getByRole("button", { name: "Create model" }).click();
  await page.getByRole("button", { name: "Add type", exact: true }).click();
  await page.getByRole("dialog", { name: "Add type" }).getByRole("button", { name: /Import from Smart Data Models/ }).click();
  await page.getByRole("button", { name: /AirQualityObserved/ }).first().click();
  await page.getByRole("radio", { name: /Adapt it/ }).click();
  await page.waitForTimeout(300);
  const wide = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(wide).toBeLessThanOrEqual(375);
});
