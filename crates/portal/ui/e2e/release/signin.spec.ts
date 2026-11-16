/**
 * The sign-in itself, typed by the browser: password, then the one-time code of the authenticator
 * this run enrolled (kaisatsu asks every person for a second factor, as jc-prod: PF-128), and the
 * sign-out. These journeys type a password, so they record no trace (a trace keeps every filled
 * value); a failure still leaves its screenshot, which shows no secret (password fields are masked
 * and the code page shows no seed).
 */
import { expect, ready, shoot, signIn, test } from "./fixtures";

test.use({ trace: "off" });

test("J001 a person signs in with a password and a one-time code and reaches the Portal", async ({ browser, baseURL, person }, info) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await signIn(page, person("admin"), "/projects/helsinki/home");
  await expect(ready(page)).toBeVisible();
  await expect(page.getByRole("button", { name: /^Signed in as / })).toBeVisible();
  await shoot(page, info, "signed-in");
  await context.close();
});

test("J002 a wrong password is refused and the person stays on the sign-in form", async ({ browser, baseURL, person }, info) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto("/api/v1/auth/login");
  await page.fill("#username", person("viewer").user);
  await page.fill("#password", "not-the-password-0");
  await page.click("#kc-login");
  await expect(page.locator("#username")).toBeVisible();
  await expect(page.locator("#input-error, .kc-feedback-text, [role=alert]").first()).toBeVisible();
  expect(await page.evaluate(() => fetch("/api/v1/auth/me").then((r) => r.status))).toBe(401);
  await shoot(page, info, "refused");
  await context.close();
});

test("J003 a wrong one-time code is refused after a right password", async ({ browser, baseURL, person }, info) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  const who = person("outsider");
  await page.goto("/api/v1/auth/login");
  await page.fill("#username", who.user);
  await page.fill("#password", who.password);
  await page.click("#kc-login");
  // A person enrolled by this run's session sign-in meets the code page.
  await expect(page.locator("#otp")).toBeVisible();
  await page.fill("#otp", "000000");
  await page.click("#kc-login");
  await expect(page.getByText(/Invalid authenticator code/i).first()).toBeVisible();
  expect(await page.evaluate(() => fetch("/api/v1/auth/me").then((r) => r.status))).toBe(401);
  await shoot(page, info, "code-refused");
  await context.close();
});

test("J004 a person signs out from the account menu and the session is gone", async ({ browser, baseURL, person }) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await signIn(page, person("viewer"), "/settings");
  await page.getByRole("button", { name: /^Signed in as / }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect.poll(() => page.evaluate(() => fetch("/api/v1/auth/me").then((r) => r.status))).toBe(401);
  await page.goto("/settings");
  await expect(page.getByRole("button", { name: "Sign in" }).first()).toBeVisible();
  await context.close();
});
