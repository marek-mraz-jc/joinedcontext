import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// Inviting a person (T-3237, PF-108): a steward invites a newcomer into a project with a role
// whose description says what it may do, sees the pending invitation with its expiry, and the
// newcomer arriving through the link is welcomed with the first step of their role.

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const PENDING = {
  id: "nora-id",
  email: "nora.nova@example.org",
  firstName: "Nora",
  lastName: "Nová",
  locale: "sk",
  enabled: true,
  emailVerified: false,
  requiredActions: ["VERIFY_EMAIL", "UPDATE_PASSWORD"],
  createdAt: "2026-10-07T08:00:00Z",
  lastSeen: null,
  pendingDeletion: null,
  invitationExpires: "2099-10-08T08:00:00Z",
};

async function mock(page: Page, verbs: string[], sent: { method: string; path: string; body: unknown }[] = []) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() !== "GET") sent.push({ method: request.method(), path: url.pathname, body: request.postDataJSON() });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "steward", name: "Stela Steward", email: "steward@example.org", roles: [] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (url.pathname.endsWith("/permissions/me")) {
      return json({
        project: "helsinki",
        grants: verbs.length ? [{ rule: { kinds: ["Person", "ContextSpace", "DataSource", "Pipeline", "Endpoint", "RoleBinding"], verbs } }] : [],
      });
    }
    if (url.pathname === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (url.pathname === "/api/v1/organization/people" && request.method() === "GET") return json({ items: [PENDING] });
    if (url.pathname === "/api/v1/organization/people") {
      return json({ person: { ...PENDING, id: "viera-id", email: "viera@example.org", firstName: "Viera" }, emailSent: true }, 201);
    }
    if (url.pathname === "/api/v1/projects/org/rolebindings") {
      return url.searchParams.has("dryRun")
        ? json({ valid: true, verdict: { ok: true, findings: [] } })
        : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-0000042", namespace: "org" }, status: { lane: "red", phase: "PendingApproval" } }, 202);
    }
    return json(LIST([]));
  });
}

test("a steward invites a newcomer into a project as an editor and sees the pending invitation", async ({ page }) => {
  const sent: { method: string; path: string; body: unknown }[] = [];
  await mock(page, ["read", "create", "update", "disable", "delete", "propose", "approve"], sent);
  await page.goto("/organization/people?lang=en");
  const pending = page.getByRole("region", { name: "Invitations not accepted yet" });
  await expect(pending.getByRole("listitem")).toContainText("The link works until");

  await page.getByRole("button", { name: "New person" }).click();
  const dialog = page.getByRole("region", { name: "New person" });
  await dialog.getByRole("textbox", { name: /E-mail/ }).fill("viera@example.org");
  await dialog.getByRole("textbox", { name: /First name/ }).fill("Viera");
  await dialog.getByRole("textbox", { name: /Last name/ }).fill("Nová");
  await dialog.getByRole("combobox", { name: /Project/ }).selectOption("helsinki");
  await dialog.getByRole("radio", { name: "Editor" }).check();
  await expect(dialog.getByRole("radiogroup", { name: "Role in the project" })).toContainText("Deletes nothing.");
  expect(await axeViolations(page)).toEqual([]);
  await dialog.getByRole("button", { name: "Create" }).click();

  await expect(page.getByText("The role Editor for viera@example.org in helsinki is proposed.")).toBeVisible();
  expect(sent.find((s) => s.path === "/api/v1/organization/people")?.body).toMatchObject({ project: "helsinki" });
});

test("a viewer arriving through the invitation's link is welcomed with their first step", async ({ page }) => {
  await mock(page, ["read"]);
  await page.goto("/projects/helsinki/home?welcome=1&lang=en");
  const welcome = page.getByRole("region", { name: "Welcome to helsinki" });
  await expect(welcome).toContainText("You are a viewer here");
  await expect(welcome.getByRole("link", { name: "Open the project's spaces" })).toHaveAttribute("href", "/projects/helsinki/spaces");
  expect(await axeViolations(page)).toEqual([]);
  await welcome.getByRole("button", { name: "Close the welcome" }).click();
  await expect(welcome).toHaveCount(0);
  await expect(page).toHaveURL(/\/projects\/helsinki\/home$/);
});
