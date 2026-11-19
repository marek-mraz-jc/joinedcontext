import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { doorsAsRest } from "./manifestReads";

// T-7380: an Organization's administrator with no project opens the Portal, lands on the
// Organization's projects page and creates the first project; a member who may not open one
// reads who does; the Organization's own People opens for its administrator, and an
// installation section names itself in the refusal. The built bundle, the API answered here.

test.beforeEach(({ page }) => doorsAsRest(page));

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const REFUSED = "opening a project here needs propose on Project, which org-admin holds (PF-65)";
const ORG_ADMIN = {
  role: "org-admin",
  binding: "bbsk-admins",
  organization: "bbsk",
  rule: { kinds: ["RoleBinding", "Group", "Organization", "Person", "Project"], verbs: ["read", "propose", "approve"] },
};

// The people routes' own verbs, which org-admin holds (helsinki-role-org-admin.yaml).
const ORG_ADMIN_PEOPLE = {
  ...ORG_ADMIN,
  rule: { kinds: ["Person"], verbs: ["read", "create", "update", "disable", "delete"] },
};

interface World {
  projects: string[];
  allowed: boolean;
  administrator: boolean;
}

async function stub(page: Page, world: World): Promise<string[]> {
  const posted: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "marek-mraz-admin", name: "Marek Mráz", email: "marek@bbsk.sk", roles: [] });
    }
    if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", logo: "" });
    if (url.pathname.endsWith("/permissions/me")) {
      return json({
        project: "org",
        bootstrap: false,
        grants: world.administrator ? [ORG_ADMIN, ORG_ADMIN_PEOPLE] : [],
        projects: { creation: world.allowed ? { allowed: true } : { allowed: false, reason: REFUSED } },
      });
    }
    if (request.method() === "POST" && url.pathname === "/api/v1/projects") {
      posted.push(request.postData() ?? "");
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-0000002a", namespace: "doprava" },
          spec: { title: "Open project doprava" },
          status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2, update: 0, delete: 0 } },
        },
        202,
      );
    }
    if (url.pathname === "/api/v1/projects") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ProjectList",
        items: world.projects.map((name) => ({ name, sample: false, organization: "bbsk", title: name === "doprava" ? "Doprava v kraji" : undefined })),
        organizations: [],
      });
    }
    if (url.pathname === "/api/v1/organization/people") {
      return json({ items: [{ id: "p1", username: "jana", firstName: "Jana", lastName: "Kováčová", email: "jana@bbsk.sk", enabled: true, emailVerified: true, requiredActions: [] }], next: null });
    }
    if (url.pathname === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (url.pathname === "/api/v1/catalogue") {
      return json({ total: 0, page: 1, pageSize: 20, datasets: [], facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] }, unavailable: [] });
    }
    return json(LIST([]));
  });
  return posted;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`the first project (${width} px)`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("an Organization's administrator with no project creates the first one from the home", async ({ page }) => {
      const posted = await stub(page, { projects: [], allowed: true, administrator: true });
      await page.goto("/?lang=en");
      await expect(page).toHaveURL(/\/projects(\?|$)/);
      await expect(page.getByRole("heading", { level: 2, name: "Create your first project" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `home-empty-may-create-${width}`);

      await page.getByRole("button", { name: "Create your first project" }).click();
      const dialog = page.getByRole("dialog", { name: "Open a project" });
      await expect(dialog).toBeVisible();
      await dialog.getByLabel(/^Name/).fill("Doprava!");
      await shot(page, `dialog-name-invalid-${width}`);
      await dialog.getByLabel(/^Name/).fill("doprava");
      await dialog.getByLabel("Display name").fill("Doprava v kraji");
      await shot(page, `dialog-filled-${width}`);
      await dialog.getByRole("button", { name: "Open project" }).click();
      await expect.poll(() => posted.length).toBe(1);
      expect(JSON.parse(posted[0])).toMatchObject({ name: "doprava", displayName: "Doprava v kraji" });
      await shot(page, `dialog-change-proposed-${width}`);
    });

    test("a member who may not open a project reads who does on /projects", async ({ page }) => {
      await stub(page, { projects: [], allowed: false, administrator: false });
      await page.goto("/projects?lang=en");
      await expect(page.getByRole("heading", { level: 2, name: "You are in no project yet" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Open the catalogue" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `projects-member-refused-${width}`);
    });

    test("with projects, /projects lists them with New project", async ({ page }) => {
      await stub(page, { projects: ["doprava", "ovzdusie"], allowed: true, administrator: true });
      await page.goto("/projects?lang=en");
      const listed = page.getByRole("list", { name: "Projects" });
      await expect(listed.getByRole("link", { name: /Doprava v kraji/ })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `projects-list-${width}`);
    });

    test("the Organization's administrator opens its People; an installation section names itself", async ({ page }) => {
      await stub(page, { projects: [], allowed: true, administrator: true });
      await page.goto("/organization/people?lang=en");
      await expect(page.getByText("belongs to the installation")).toHaveCount(0);
      await expect(page.getByText("Jana", { exact: false }).first()).toBeVisible();
      await shot(page, `organization-people-own-admin-${width}`);
      await page.goto("/organization/brokers?lang=en");
      await expect(page.getByText("The Brokers section belongs to the installation: only its administrators open it.")).toBeVisible();
      await shot(page, `organization-brokers-refused-${width}`);
    });
  });
}
