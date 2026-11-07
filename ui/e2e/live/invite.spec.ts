/**
 * T-3237, PF-108 — inviting a person on dev. The steward invites a throwaway address into
 * helsinki as a viewer: the invitation names the project, the role is proposed as a change, and
 * the pending invitation is listed with its expiry, sent again and revoked. The proposed change is
 * rejected afterwards and the account is gone, so dev is left as it was. The second journey is the
 * arrival: demo.viewer opens the page the invitation's link ends on and is greeted as a viewer
 * with their first step. The e-mail itself is the realm's and is not walked here.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, proposedChange, reject, signIn } from "./portal";

test.setTimeout(300_000);

const PROJECT = "helsinki";

test("a steward invites a newcomer into a project as a viewer, sends it again and revokes it", async ({ browser }) => {
  const email = `journey-invite-${Date.now()}@example.org`;
  const { context, page } = await signIn(browser, STEWARD, `/organization/people?lang=en`);
  let change = "";
  let revoked = false;
  try {
    await page.getByRole("button", { name: "New person" }).click();
    const dialog = page.getByRole("region", { name: "New person" });
    await dialog.getByRole("textbox", { name: /E-mail/ }).fill(email);
    await dialog.getByRole("textbox", { name: /First name/ }).fill("Journey");
    await dialog.getByRole("textbox", { name: /Last name/ }).fill("Invite");
    await dialog.getByRole("combobox", { name: /Project/ }).selectOption(PROJECT);
    const roles = dialog.getByRole("radiogroup", { name: "Role in the project" });
    await expect(roles.getByRole("radio", { name: "Viewer" })).toBeChecked();
    await expect(roles).toContainText("Changes nothing.");
    await dialog.getByRole("button", { name: "Create" }).click();

    // A realm without e-mail hands a temporary password once: close it, it is not this journey's.
    const password = page.getByRole("dialog", { name: /temporary password/i });
    if (await password.isVisible({ timeout: 5_000 }).catch(() => false)) {
      await password.getByRole("button").first().click();
    }
    await expect(page.getByText(`The role Viewer for ${email} in ${PROJECT} is proposed.`)).toBeVisible({ timeout: 60_000 });
    change = await proposedChange(page);

    await page.reload({ waitUntil: "load" });
    await page.getByRole("searchbox", { name: /Search/ }).fill(email);
    await page.getByRole("button", { name: /Search/ }).last().click();
    const pending = page.getByRole("region", { name: "Invitations not accepted yet" });
    const row = pending.getByRole("listitem").filter({ hasText: email });
    await expect(row).toBeVisible({ timeout: 60_000 });
    await expect(row).toContainText(/The link works until|did not record when the link expires/);

    await row.getByRole("button", { name: `Send the invitation to ${email} again` }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Send the invitation again" }).click();
    await expect(pending.getByText(new RegExp(`sent the invitation to ${email.replace(/[.]/g, "\\.")} again`))).toBeVisible({ timeout: 60_000 });

    await row.getByRole("button", { name: `Revoke the invitation of ${email}` }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Revoke" }).click();
    await expect(pending.getByText(`The invitation of ${email} is revoked and the account deleted.`)).toBeVisible({ timeout: 60_000 });
    revoked = true;
  } finally {
    if (change) await reject(page, "org", change, "Rejected by a live journey: the invitation was only proved, then revoked.");
    await context.close();
  }
  expect(revoked, `${email} is still in the realm: revoke it under Organization → People`).toBe(true);
});

test("demo.viewer arriving through the invitation's link is welcomed as a viewer, one click from the data", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/home?welcome=1&lang=en`);
  try {
    const welcome = page.getByRole("region", { name: `Welcome to ${PROJECT}` });
    await expect(welcome).toBeVisible({ timeout: 60_000 });
    await expect(welcome).toContainText("You are a viewer here");
    await welcome.getByRole("link", { name: "Open the project's spaces" }).click();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/spaces`));
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
  } finally {
    await context.close();
  }
});
