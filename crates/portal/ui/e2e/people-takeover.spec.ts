import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// PF-93 (S-5-03): a people administrator acting on a person who holds a Policy, App or Endpoint
// grant they could not change may only reset the password by e-mail and remove the second factor
// with an e-mail. The person page shows the server's reason for a refused e-mail change and for a
// reset the realm cannot mail, and the success line for a reset that went by e-mail. Each state is
// shot at desktop and phone width.

const OLA = {
  id: "ola-id",
  email: "ola.kovacova@example.org",
  firstName: "Ola",
  lastName: "Kovacova",
  locale: "sk",
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-10-01T08:00:00Z",
  lastSeen: "2026-11-14T09:30:00Z",
  pendingDeletion: null,
  invitationExpires: null,
};
const HOLDS = "ola.kovacova@example.org holds Policy helsinki/air-read";

type Mail = "sent" | "none";

async function mock(page: Page, mail: Mail, sent: string[]) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: status < 300 ? "application/json" : "application/problem+json",
        body: JSON.stringify(body),
      });
    const refused = (detail: string) => json({ type: "about:blank", title: "Forbidden", status: 403, detail }, 403);
    if (request.method() !== "GET") sent.push(`${request.method()} ${url.pathname}`);
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "pia", name: "Pia People", email: "pia@example.org", roles: [] });
    }
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ rule: { kinds: ["Person"], verbs: ["read", "create", "update", "disable", "delete"] } }] });
    }
    if (url.pathname === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (url.pathname === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (url.pathname === "/api/v1/organization/people/ola-id" && request.method() === "GET") {
      return json({
        person: OLA,
        groups: [],
        platformRoles: [],
        appRoles: [{ project: "helsinki", app: "air-board", role: "viewer", via: { user: OLA.email } }],
      });
    }
    if (url.pathname === "/api/v1/organization/people/ola-id" && request.method() === "PATCH") {
      return refused(
        `${HOLDS}, which you cannot change; only a password reset by e-mail or a second-factor removal with an e-mail is open to you (PF-93)`,
      );
    }
    if (url.pathname === "/api/v1/organization/people/ola-id/readable") return json({ datasets: [] });
    if (url.pathname === "/api/v1/organization/people/ola-id/reset-password") {
      return mail === "sent"
        ? json({ emailSent: true }, 202)
        : refused(`${HOLDS}; the realm cannot send mail, so only a platform admin can reset them (PF-93)`);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shoot(page: Page, name: string) {
  for (const [width, height] of [
    [1440, 900],
    [400, 860],
  ] as const) {
    await page.setViewportSize({ width, height });
    await page.screenshot({ path: test.info().outputPath(`${name}-${width}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function reset(page: Page) {
  await page.getByRole("button", { name: "Reset password" }).click();
  await expect(page.getByRole("alertdialog", { name: "Reset the password of Ola Kovacova?" })).toBeVisible();
  await page.getByTestId("confirm-accept").click();
}

test("an e-mail change of a person holding a grant the admin cannot change is refused with the reason", async ({ page }) => {
  const sent: string[] = [];
  await mock(page, "sent", sent);
  await page.goto("/organization/people/ola-id?lang=en");
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit Ola Kovacova" });
  await dialog.getByLabel("E-mail").fill("ola.new@example.org");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog.getByRole("alert")).toContainText("only a password reset by e-mail");
  await expect(dialog).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
  expect(sent).toEqual(["PATCH /api/v1/organization/people/ola-id"]);
  await shoot(page, "people-takeover-email-refused");
});

test("a reset the realm cannot mail is refused and no temporary password appears", async ({ page }) => {
  const sent: string[] = [];
  await mock(page, "none", sent);
  await page.goto("/organization/people/ola-id?lang=en");
  await reset(page);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(
    page.getByRole("alert").filter({ hasText: "the realm cannot send mail, so only a platform admin can reset them" }),
  ).toBeVisible();
  await expect(page.getByText(/temporary password/i)).toHaveCount(0);
  expect(await axeViolations(page)).toEqual([]);
  await shoot(page, "people-takeover-reset-refused");
});

test("a reset by e-mail of a person holding such a grant says the link went", async ({ page }) => {
  const sent: string[] = [];
  await mock(page, "sent", sent);
  await page.goto("/organization/people/ola-id?lang=en");
  await expect(page.getByRole("heading", { name: "Ola Kovacova", level: 1 })).toBeVisible();
  await expect(page.getByRole("button", { name: "Reset password" })).toBeEnabled();
  await shoot(page, "people-takeover-person");
  await reset(page);
  await expect(
    page.getByRole("status").filter({ hasText: "The realm e-mailed the person a link to set a new password." }),
  ).toBeVisible();
  expect(sent).toEqual(["POST /api/v1/organization/people/ola-id/reset-password"]);
  await shoot(page, "people-takeover-reset-mailed");
});
