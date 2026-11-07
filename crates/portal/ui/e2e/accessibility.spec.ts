import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The accessibility statement (T-5108, Decree 78/2020): a visitor reaches it from the footer of
// every public page with the keyboard alone, reads it in English and Slovak at desktop and
// phone width, and axe finds nothing on it.
async function visitor(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/branding")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ organisation: "Mesto Banská Bystrica", contactEmail: "pristupnost@example.sk" }),
      });
      return;
    }
    const unauthorized = path.endsWith("/auth/me");
    await route.fulfill({
      status: unauthorized ? 401 : 404,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
    });
  });
}

test("a keyboard-only visitor goes from the glossary's footer to the statement", async ({ page }) => {
  await visitor(page);
  await page.goto("/glossary?lang=en");
  await expect(page.getByRole("heading", { level: 1, name: "Glossary" })).toBeVisible();
  const link = page.getByRole("contentinfo").getByRole("link", { name: "Accessibility" });
  // Tab until the footer link holds focus: no mouse, and the walk must not get stuck on the way.
  for (let presses = 0; presses < 40; presses += 1) {
    await page.keyboard.press("Tab");
    if (await link.evaluate((element) => element === document.activeElement)) break;
  }
  await expect(link).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/accessibility$/);
  await expect(page.getByRole("heading", { level: 1, name: "Accessibility statement" })).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
});

for (const [lang, title, width] of [
  ["en", "Accessibility statement", 1440],
  ["sk", "Vyhlásenie o prístupnosti", 400],
] as const) {
  test(`the statement reads in ${lang} at ${width} px with no axe finding`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 });
    await visitor(page);
    await page.goto(`/accessibility?lang=${lang}`);
    await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
    await expect(page.getByRole("link", { name: "pristupnost@example.sk" })).toHaveAttribute("href", "mailto:pristupnost@example.sk");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "no sideways scroll").toBe(true);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: info.outputPath(`accessibility-${lang}-${width}.png`), fullPage: true });
  });
}
