import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-2605, UI-75: the Organization page in a real browser, the API answered in the browser as the
// other mocked specs do. What a browser adds over the vitest suite: the router's own addresses,
// the tab list driven by the keyboard, and a deletion that sends exactly one DELETE.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "ida.admin",
  name: "Ida Admin",
  email: "ida@hel.fi",
  roles: ["portal-editor"],
};

// A group names its members by Keycloak id (ADR-N-061): the picker finds people in the directory.
const PETRA = { id: "5f0c2a9e-4b1d-4c3e-9a7f-2d8e6b1c0a43", email: "petra@hel.fi", firstName: "Petra", lastName: "Kráľ", enabled: true, emailVerified: true, requiredActions: [] };

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "hel", namespace: "org" },
  spec: {
    domain: "hel.fi",
    locales: ["fi", "en"],
    defaultLocale: "fi",
    projects: { creation: "org-admin", visibility: "organization" },
  },
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: {
        kinds: ["RoleBinding", "Role", "Group", "Organization", "Project", "ServiceAccount", "Person"],
        verbs: ["propose", "approve", "delete"],
      },
    },
  ],
};

const GROUP = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Group",
  metadata: { name: "air-quality-steward", namespace: "org" },
  spec: { members: [{ user: "jana@hel.fi" }] },
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const { pathname: path, search } = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path}${search}`);
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-000002a1", namespace: "helsinki" },
          status: { lane: "red", phase: "PendingApproval" },
        },
        202,
      );
    }
    if (path.endsWith("/permissions/me")) return json(ADMIN);
    if (path === "/api/v1/organization/people") {
      const typed = (new URL(request.url()).searchParams.get("search") ?? "").toLowerCase();
      const found = [PETRA].filter((p) => `${p.firstName} ${p.lastName} ${p.email}`.toLowerCase().includes(typed));
      return json({ items: found, total: found.length });
    }
    if (path === "/api/v1/organization/people/names") return json({ items: [{ id: PETRA.id, name: "Petra Kráľ" }] });
    if (path === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (path === "/api/v1/projects/helsinki") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Project",
        metadata: {},
        spec: {},
        status: { usage: { contextSpaces: 2 } },
      });
    }
    if (path === "/api/v1/projects/org/organizations") return json(LIST([ORGANIZATION]));
    if (path === "/api/v1/projects/org/groups/air-quality-steward") return json(GROUP);
    if (path === "/api/v1/projects/org/roles") {
      return json(
        LIST([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Role",
            metadata: { name: "org-admin", namespace: "org" },
            spec: { rules: [{ kinds: ["Project"], verbs: ["propose", "approve", "delete"] }] },
          },
        ]),
      );
    }
    return json(LIST([]));
  });
  return { writes };
}

// T-4949, UI-16: each field of a group's page says what to enter, with an example; the live
// walker found five without a hint on dev.
for (const width of [1440, 400]) {
  test(`a group's fields each carry a hint with an example at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const { writes } = await stubApi(page);
    await page.goto("/organization/groups/air-quality-steward?lang=en");
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    const email = page.getByRole("combobox", { name: "Person", exact: true });
    await expect(email).toHaveAccessibleDescription(/e\.g\. jana\.novak@example\.org/);
    await expect(page.getByLabel("Where")).toHaveAccessibleDescription(/e\.g\. helsinki/);
    await page.getByLabel("Where").selectOption("helsinki");
    await expect(page.getByLabel("Context space")).toHaveAccessibleDescription(/narrow the role to one context space/);
    for (const label of ["Project", "Application"]) {
      await expect(page.getByLabel(label, { exact: true })).toHaveAccessibleDescription(/e\.g\. /);
    }
    for (const role of await page.getByLabel("Role", { exact: true }).all()) {
      await expect(role).toHaveAccessibleDescription(/e\.g\. /);
    }
    // The flow still works: a member found by their e-mail and added is one proposal (T-4947):
    // proposeChecked sends the dry run, then the change itself.
    await email.fill("petra@hel.fi");
    await page.getByRole("option", { name: /Petra Kráľ/ }).click();
    // The list stays open for a second pick; Escape closes it over the button.
    await email.press("Escape");
    await page.getByRole("button", { name: "Add member" }).click();
    const group = "PUT /api/v1/projects/org/groups/air-quality-steward";
    await expect.poll(() => writes).toEqual([`${group}?dryRun=All`, group]);
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(sideways, "the page scrolls sideways").toBeLessThanOrEqual(0);
    expect(await axeViolations(page)).toEqual([]);
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/group-hints-${width}.png`, fullPage: true });
  });
}

test.describe("the Organization page", () => {
  // The sections are the Administration sidebar's links since T-4831, no longer a tab list.
  test("opens on Settings and walks its sections from the sidebar, each at its own address", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization?lang=en");
    await expect(page).toHaveURL(/\/organization\/settings$/);
    await expect(page.getByRole("heading", { level: 1, name: "Administration" })).toBeVisible();
    await expect(page.getByText("Only an organization administrator opens projects.")).toBeVisible();
    await expect(page.getByRole("link", { name: "Settings", exact: true })).toHaveAttribute("aria-current", "page");

    await page.getByRole("link", { name: "People", exact: true }).click();
    await expect(page).toHaveURL(/\/organization\/people$/);
    await expect(page.getByRole("link", { name: "People", exact: true })).toHaveAttribute("aria-current", "page");
    await page.getByRole("link", { name: "Roles", exact: true }).click();
    await expect(page).toHaveURL(/\/organization\/roles$/);
    await expect(page.getByRole("row", { name: /org-admin/ }).getByText("seeded")).toBeVisible();
  });

  test("deleting a project lists what goes with it and sends one DELETE after the name is typed", async ({
    page,
  }) => {
    const { writes } = await stubApi(page);
    await page.goto("/organization/projects?lang=en");
    await page.getByRole("button", { name: "Delete project helsinki" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("2 × Context Spaces")).toBeVisible();
    await dialog.getByRole("textbox").fill("helsinki");
    await dialog.getByRole("button", { name: "Propose deleting the project" }).click();
    await expect(dialog.getByText("chg-000002a1")).toBeVisible();
    expect(writes).toEqual(["DELETE /api/v1/projects/helsinki"]);
  });

  test("a tab's create form is a page of its own", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/groups/new?lang=en");
    await expect(page.getByRole("heading", { name: "New group" })).toBeVisible();
  });

  test("has no axe violations", async ({ page }) => {
    await stubApi(page);
    await page.goto("/organization/settings?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Administration" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
