/**
 * T-2729 — the /login page on dev, walked by a person (UI-16, UI-46, AP-29).
 *
 * The other journeys pass /login on the way in and never look at it. This one opens it on
 * purpose. On dev the edge answers a person without a session with Keycloak's form before the
 * page renders (`unauth_action: auth`, deployment `components/portal/apisix-plugins.yaml`), and
 * Keycloak sends them back to the address they opened, /login included; a Portal signing in
 * by itself shows its own Sign in button first. Either way the person lands where the session was
 * asked for, and Sign out ends the session so the next page asks again. The refusals are the
 * ones a sign-in page owes: a wrong password gets no session, and a `redirect_to` that points off
 * the Portal is not followed.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, portalReady } from "./portal";

test.setTimeout(180_000);

const signInButton = (page: Page) => page.getByRole("button", { name: "Sign in" });

/** Past the Portal's own Sign in button, when this front shows one, to Keycloak's form. */
async function toKeycloak(page: Page): Promise<void> {
  const form = page.locator("#username");
  await expect(form.or(signInButton(page)).first()).toBeVisible({ timeout: 60_000 });
  if (!(await form.count())) {
    await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), signInButton(page).click()]);
  }
  await expect(form).toBeVisible({ timeout: 60_000 });
}

/** The Keycloak form, filled and submitted once. */
async function keycloak(page: Page, user: string, password: string): Promise<void> {
  await page.fill("#username", user);
  await page.fill("#password", password);
  await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), page.click("#kc-login")]);
}

/** Whether the browser holds a Portal session, as the Portal itself answers. */
async function signedIn(page: Page): Promise<boolean> {
  return (await page.request.get("/api/v1/auth/me")).ok();
}

test("a person signs in from /login to the page they asked for, and signs out", async ({ browser }) => {
  expect(STEWARD.password, "PORTAL_PASSWORD is set").not.toBe("");
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto("/login?lang=en&redirect_to=%2Fprojects%2Fhelsinki%2Fspaces", { waitUntil: "load" });
    expect(await signedIn(page), "nobody is signed in yet").toBe(false);
    await toKeycloak(page);
    await keycloak(page, STEWARD.user, STEWARD.password);

    await expect(portalReady(page)).toBeVisible({ timeout: 60_000 });
    await expect(page, "the sign-in lands where the session was asked for").toHaveURL(/\/projects\/helsinki\/spaces/);
    await expect(signInButton(page), "nobody signed in is offered Sign in").toHaveCount(0);
    expect(await signedIn(page)).toBe(true);

    await page.getByRole("button", { name: /^Signed in as / }).click();
    await Promise.all([
      page.waitForURL(() => true, { waitUntil: "load" }),
      page.getByRole("menuitem", { name: "Sign out" }).click(),
    ]);
    await expect.poll(() => signedIn(page), { timeout: 30_000, message: "Sign out ends the session" }).toBe(false);

    await page.goto("/projects/helsinki/spaces?lang=en", { waitUntil: "load" });
    await expect(
      page.locator("#username").or(signInButton(page)).first(),
      "a page of the project asks for a sign-in again",
    ).toBeVisible({ timeout: 60_000 });
    await expect(portalReady(page)).toHaveCount(0);
  } finally {
    await context.close();
  }
});

test("a wrong password gets no session, and an address off the Portal is not followed", async ({ browser }) => {
  expect(STEWARD.password, "PORTAL_PASSWORD is set").not.toBe("");
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto("/login?lang=en&redirect_to=%2F%2Fexample.org%2Fphish", { waitUntil: "load" });
    await toKeycloak(page);
    await keycloak(page, STEWARD.user, `${STEWARD.password}-wrong`);
    await expect(page.locator("#username"), "Keycloak keeps the form and refuses").toBeVisible();
    await expect(portalReady(page)).toHaveCount(0);
    expect(await signedIn(page)).toBe(false);

    await keycloak(page, STEWARD.user, STEWARD.password);
    await expect(portalReady(page)).toBeVisible({ timeout: 60_000 });
    const landed = new URL(page.url());
    expect(landed.host, "the sign-in never leaves the Portal").not.toBe("example.org");
    expect(landed.pathname, "and does not stay on /login").not.toBe("/login");
  } finally {
    await context.close();
  }
});
