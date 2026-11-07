import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// `vite preview` serves the built bundle with no portal API behind it, so `/api/v1/auth/me`
// never answers with a session and every journey here is the anonymous one. The authenticated
// journeys arrive with the task that runs the Rust binary in ci-full.
//
// `?lang=` is the first source i18next's detector consults, so nothing depends on the
// browser's own locale.
test.describe("portal", () => {
  test("sends an anonymous visitor to the login route", async ({ page }) => {
    await page.goto("/?lang=sk");

    await expect(page).toHaveURL(/\/login/);
    // The heading is the installation's brand mark since the runtime branding endpoint landed
    // (UI-30), not the static `app.title`. `vite preview` serves the bundle with no portal API,
    // so /api/v1/branding is refused and the shell paints NEUTRAL_BRANDING — whose instanceName
    // is "joinedcontext". `exact`, because that name is a prefix of every branded one.
    await expect(
      page.getByRole("heading", { name: "joinedcontext", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Prihlásiť sa" })).toBeVisible();
  });

  test("switches language and keeps the document language in step", async ({ page }) => {
    await page.goto("/login?lang=sk");

    await expect(page.locator("html")).toHaveAttribute("lang", "sk");
    await expect(page.getByRole("button", { name: "Prihlásiť sa" })).toBeVisible();

    await page.getByRole("button", { name: "Jazyk" }).click();
    await page.getByRole("menuitem", { name: "English" }).click();

    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
  });

  test("starts the server-side code flow when sign-in is pressed", async ({ page }) => {
    await page.goto("/login?lang=en");

    const request = page.waitForRequest((r) => r.url().includes("/api/v1/auth/login"));
    await page.getByRole("button", { name: "Sign in" }).click();

    const url = new URL((await request).url());
    expect(url.pathname).toBe("/api/v1/auth/login");
    expect(url.searchParams.get("redirect_to")).toBe("/");
  });

  test("the login route has no axe violations", async ({ page }) => {
    await page.goto("/login?lang=sk");
    await expect(page.getByRole("button", { name: "Prihlásiť sa" })).toBeVisible();

    expect(await axeViolations(page)).toEqual([]);
  });
});
