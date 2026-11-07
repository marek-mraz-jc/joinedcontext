import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// The platform's words (T-3236): a visitor opens a word's entry from its link and reads the
// definition and the example, without a session.
test("a visitor reads a word's entry in the glossary", async ({ page }) => {
  await page.route("**/api/v1/**", async (route) => {
    const unauthorized = new URL(route.request().url()).pathname.endsWith("/auth/me");
    await route.fulfill({
      status: unauthorized ? 401 : 404,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
    });
  });
  await page.goto("/glossary?lang=en");
  await expect(page.getByRole("heading", { level: 1, name: "Glossary" })).toBeVisible();
  const entry = page.locator("#term-entityType");
  await expect(entry.getByText("Entity type", { exact: true })).toBeVisible();
  await expect(entry.getByText(/^Example: AirQualityObserved/)).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
});
