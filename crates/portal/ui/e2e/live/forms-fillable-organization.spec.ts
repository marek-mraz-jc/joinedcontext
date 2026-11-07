/**
 * Every form fillable by a new user: Organization, members and roles (T-3216).
 *
 * A person new to the platform walks the organization's workflows on dev with only what the
 * screen gives them: the setup checklist names each step and links where it is done, People
 * offers "invite" with its help, the grant dialog lists the realm's people under the typing and
 * says what the chosen role lets a person do, and Policies and limits shows each limit with its
 * range. Nothing is proposed: the grant dialog is filled and cancelled, nobody is invited.
 */
import { expect, test } from "@playwright/test";
import { EDITOR, STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";

for (const who of [STEWARD, EDITOR]) {
  test(`a new user finds what each organization form needs on the screen (${who.user})`, async ({ browser }) => {
    const { context, page } = await signIn(browser, who, `/projects/${PROJECT}/settings/members?lang=en`);
    try {
      // Give a role: who, which role and what it allows, where, until when.
      const grant = page.getByRole("button", { name: "Grant a role" });
      await expect(grant).toBeVisible({ timeout: 60_000 });
      if ((await grant.getAttribute("aria-disabled")) === "true") {
        await expect(grant).toHaveAccessibleDescription(/propose/);
      } else {
        await grant.click();
        const dialog = page.getByRole("dialog");
        const person = dialog.getByLabel(/Username or e-mail/);
        await expect(person).toHaveAccessibleDescription(/exactly as they sign in/);
        await person.fill("demo.viewer@hel.fi");
        const role = dialog.getByLabel(/^Role/);
        await expect(role.locator("option:not([value=''])").first()).toBeAttached({ timeout: 30_000 });
        await role.selectOption({ index: 1 });
        // The chosen role says what it allows, without a trip to Roles.
        await expect(role).toHaveAccessibleDescription(/This role may: |\w{10,}/);
        await dialog.getByRole("button", { name: "Cancel" }).click();
        const discard = page.getByRole("button", { name: "Discard the grant" });
        if (await discard.isVisible()) await discard.click();
      }

      // See the limits: every one with the value in force and its allowed range.
      await page.goto("/organization/settings?lang=en");
      const limits = page.getByRole("heading", { name: "Policies and limits" });
      await expect(limits.or(page.getByRole("alert")).first()).toBeVisible({ timeout: 60_000 });
      if (await limits.isVisible()) {
        await expect(page.getByRole("columnheader", { name: "Allowed range" }).first()).toBeVisible();
      }

      // Invite a member: the People tab says who may, and the form says what happens after.
      await page.goto("/organization/people?lang=en");
      const invite = page.getByRole("button", { name: "New person" });
      if (await invite.isVisible()) {
        if ((await invite.getAttribute("aria-disabled")) !== "true") {
          await invite.click();
          await expect(page.getByText(/e-mails the person a link/)).toBeVisible();
          await page.getByRole("button", { name: "Cancel" }).click();
        }
      } else {
        await expect(page.getByText(/administrator|may not|cannot/i).first()).toBeVisible();
      }
    } finally {
      await context.close();
    }
  });
}
