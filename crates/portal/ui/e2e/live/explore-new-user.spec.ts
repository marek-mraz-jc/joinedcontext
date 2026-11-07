/**
 * T-3218 — a person new to the platform explores a type with only what the screen gives them:
 * they pick the space, keep the endpoint the page chose, and pick a type the list says the
 * endpoint grants them, never one marked as not granted; the grid then holds rows. Read only.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

test("a new user picks a type the endpoint grants and reads its rows", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/explore?lang=en");
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Explore data" })).toBeVisible({ timeout: 60_000 });
    await page.getByRole("button", { name: "Helsinki city context" }).click();
    const types = page.locator("#explore-type");
    // The endpoint's grant arrives, and the types it does not grant say so.
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
    await expect(page.getByText(/No grant names/)).toHaveCount(0);
  } finally {
    await context.close();
  }
});
