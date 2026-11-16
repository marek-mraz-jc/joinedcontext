import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7071, journey 5: a name taken (the realm already has the e-mail), no permission (a person who
// does not administer the organization), and an empty Organization (no project yet). Each answers
// with the server's words and one next step.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

interface Setup {
  admin: boolean;
  projects: string[];
  writes: string[];
}

async function stub(page: Page, setup: Setup): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "a1", username: "ida", name: "Ida Admin", email: "ida@hel.fi", roles: [] });
    if (path === "/api/v1/projects") return json(list(setup.projects.map((name) => ({ name }))));
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path.endsWith("/permissions/me")) {
      const rule = setup.admin
        ? { kinds: ["Person", "RoleBinding", "Role", "Group", "Organization", "Project"], verbs: ["read", "create", "update", "disable", "delete", "propose", "approve"] }
        : { kinds: ["ContextSpace"], verbs: ["read"] };
      return json({ project: "org", bootstrap: false, grants: [{ role: setup.admin ? "org-admin" : "viewer", binding: "b", scope: "organization", rule }] });
    }
    if (path === "/api/v1/organization/people") {
      if (request.method() === "POST") {
        setup.writes.push(`POST ${path}`);
        return route.fulfill({
          status: 409,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "Conflict", status: 409, detail: "petra.kral@example.org already belongs to a person of the realm" }),
        });
      }
      setup.writes.push(`GET ${path}`);
      return json({ items: [] });
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  // From the top: a full-page shot of a scrolled page draws the fixed top bar mid-page.
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test(`a taken e-mail keeps the form open with the server's reason at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const setup: Setup = { admin: true, projects: ["helsinki"], writes: [] };
    await stub(page, setup);
    await page.goto("/organization/people?lang=en");
    await page.getByRole("button", { name: "New person" }).click();
    await page.getByRole("textbox", { name: /^E-mail/ }).fill("petra.kral@example.org");
    await page.getByRole("textbox", { name: /^First name/ }).fill("Petra");
    await page.getByRole("textbox", { name: /^Last name/ }).fill("Kráľ");
    await page.getByRole("button", { name: "Create person" }).click();
    // The reason sits on the E-mail field, with where to find the person instead.
    const email = page.getByRole("textbox", { name: /^E-mail/ });
    await expect(email).toHaveAccessibleDescription(/already belongs to a person of the realm\. Find this person in the People list/);
    await expect(email).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByRole("button", { name: "Create person" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-error-taken-${width}`);
  });

  test(`a person without the right reads whom to ask, and nothing is fetched at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const setup: Setup = { admin: false, projects: ["helsinki"], writes: [] };
    await stub(page, setup);
    await page.goto("/organization/people?lang=en");
    await expect(page.getByText("This page is for organization administrators.", { exact: false })).toBeVisible();
    await expect(page.getByText("Ask an organization administrator")).toBeVisible();
    expect(setup.writes).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-error-denied-${width}`);
  });

  test(`an Organization with no project says how the first one comes at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const setup: Setup = { admin: true, projects: [], writes: [] };
    await stub(page, setup);
    await page.goto("/organization/projects?lang=en");
    await expect(page.getByText("No project yet")).toBeVisible();
    await expect(page.getByText("Open the first one with New project.")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-error-empty-${width}`);
  });
}
