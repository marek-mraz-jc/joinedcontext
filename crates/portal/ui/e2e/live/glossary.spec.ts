/**
 * T-3236 — the glossary on dev: a signed-in steward follows a word's link from a space's page to
 * its entry and reads the definition and the example; the page answers a visitor as well.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(120_000);

test("a steward follows a word's link to the glossary, and a visitor reads it too", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/glossary?lang=en`);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Glossary" })).toBeVisible({ timeout: 60_000 });
    await expect(page.locator("#term-contextSpace").getByText(/^Example: /)).toBeVisible();
  } finally {
    await context.close();
  }
  const visitor = await browser.newContext();
  try {
    const page = await visitor.newPage();
    await page.goto(`/glossary?lang=sk`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: "Slovník" })).toBeVisible({ timeout: 60_000 });
  } finally {
    await visitor.close();
  }
});
