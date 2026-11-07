// Settings on dev (T-6344): the account menu opens /settings, which lists the person's own settings
// and their project's; Administration stays away from someone who does not administer the platform.
import { expect, test } from "@playwright/test";
import { VIEWER, goSignedIn } from "./portal";

test("a viewer opens Settings from the account menu and finds no platform group", async ({ page }) => {
  await goSignedIn(page, VIEWER, "/?lang=en");
  await page.getByRole("button", { name: /Signed in as/ }).click();
  await expect(page.getByRole("menuitem", { name: "Administration" })).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Settings" }).click();
  await expect(page).toHaveURL(/\/settings(\?|$)/);
  await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
  await expect(page.getByRole("link", { name: "My AI connectors" })).toBeVisible();
  await expect(page.getByText("Platform", { exact: true })).toHaveCount(0);
  await page.goto("/settings?lang=en");
  await expect(page.getByRole("link", { name: "Accessibility", exact: true })).toHaveAttribute("href", "/accessibility");
});
