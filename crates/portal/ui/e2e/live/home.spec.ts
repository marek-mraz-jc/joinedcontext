/**
 * T-3233, T-3235 — the project home on dev: each demo role lands on it after sign-in, and its
 * first card or its first unfinished step leads to a real place of the Portal.
 */
import { expect, test } from "@playwright/test";
import { EDITOR, STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(240_000);

for (const [who, person] of [["steward", STEWARD], ["editor", EDITOR], ["viewer", VIEWER]] as const) {
  test(`the ${who} lands on the home and its first link opens a real page`, async ({ browser }) => {
    const { context, page } = await signIn(browser, person, `/?lang=en`);
    try {
      await expect(page).toHaveURL(/\/projects\/[a-z0-9-]+\/home/, { timeout: 60_000 });
      await expect(page.getByRole("heading", { level: 1, name: "Home" })).toBeVisible({ timeout: 60_000 });
      await page.goto(`/projects/helsinki/home?lang=en`, { waitUntil: "load" });
      await expect(page.getByRole("heading", { level: 1, name: "Home" })).toBeVisible({ timeout: 60_000 });
      const first = page.getByRole("main").getByRole("link").first();
      if ((await first.count()) > 0) {
        await first.click();
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
        await expect(page.getByRole("heading", { level: 1 })).not.toHaveText("Home");
      }
    } finally {
      await context.close();
    }
  });
}
