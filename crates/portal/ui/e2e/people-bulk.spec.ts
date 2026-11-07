import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// A department leaving (T-5606): an administrator ticks the people, confirms once with the count,
// and one removal Change takes all of them out of every Group and RoleBinding. A person the server
// refuses is named in the alert. Each state is shot at desktop and phone width.

const person = (id: string, first: string, last: string) => ({
  id,
  email: `${first.toLowerCase()}.${last.toLowerCase()}@example.org`,
  firstName: first,
  lastName: last,
  locale: "sk",
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-10-01T08:00:00Z",
  lastSeen: "2026-10-24T09:30:00Z",
  pendingDeletion: null,
  invitationExpires: null,
});
const PEOPLE = [
  person("anna-id", "Anna", "Horvathova"),
  person("boris-id", "Boris", "Kral"),
  person("cyril-id", "Cyril", "Mak"),
  person("dana-id", "Dana", "Nova"),
];
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000077", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 2 } },
};

async function mock(page: Page, refuse: boolean, sent: { path: string; body: unknown }[]) {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: status < 300 ? "application/json" : "application/problem+json",
        body: JSON.stringify(body),
      });
    if (request.method() !== "GET") sent.push({ path: url.pathname, body: request.postDataJSON() });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "steward", name: "Stela Steward", email: "steward@example.org", roles: [] });
    }
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ rule: { kinds: ["Person", "RoleBinding"], verbs: ["read", "create", "update", "disable", "delete", "propose", "approve"] } }] });
    }
    if (url.pathname === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (url.pathname === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (url.pathname === "/api/v1/organization/people") return json({ items: PEOPLE });
    if (url.pathname === "/api/v1/organization/people/remove") {
      return refuse
        ? json(
            {
              type: "about:blank",
              title: "Forbidden",
              status: 403,
              detail: "dana.nova@example.org holds a right you do not, through the binding 'finance-approvers' (PF-93)",
            },
            403,
          )
        : json(CHANGE, 202);
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

test("an administrator removes a department in one Change after one confirm", async ({ page }) => {
  const sent: { path: string; body: unknown }[] = [];
  await mock(page, false, sent);
  await page.goto("/organization/people?lang=en");
  await expect(page.getByText("Tick people to disable or delete several at once")).toBeVisible();
  await shoot(page, "people-bulk-empty");

  for (const name of ["Anna Horvathova", "Boris Kral", "Cyril Mak"]) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }
  const toolbar = page.getByRole("toolbar", { name: "3 selected" });
  await expect(toolbar).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
  await shoot(page, "people-bulk-selected");

  await toolbar.getByRole("button", { name: "Delete" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Delete 3 people?" });
  await expect(dialog).toContainText("One change takes all of them out of every group");
  await shoot(page, "people-bulk-confirm");
  await dialog.getByTestId("confirm-accept").click();

  await expect(page.getByText(/chg-0000077/)).toBeVisible();
  await expect(toolbar).toHaveCount(0);
  expect(sent).toEqual([{ path: "/api/v1/organization/people/remove", body: { ids: ["anna-id", "boris-id", "cyril-id"] } }]);
  await shoot(page, "people-bulk-change");
});

test("a removal the server refuses names the person and changes nothing", async ({ page }) => {
  const sent: { path: string; body: unknown }[] = [];
  await mock(page, true, sent);
  await page.goto("/organization/people?lang=en");
  await page.getByRole("checkbox", { name: "Select everyone on this page" }).check();
  await page.getByRole("toolbar", { name: "4 selected" }).getByRole("button", { name: "Delete" }).click();
  await page.getByRole("alertdialog", { name: "Delete 4 people?" }).getByTestId("confirm-accept").click();

  await expect(page.getByRole("alert")).toContainText("dana.nova@example.org holds a right you do not");
  await expect(page.getByRole("toolbar", { name: "4 selected" })).toBeVisible();
  await shoot(page, "people-bulk-refused");
});
