import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-2606, UI-76: Project settings in a real browser, the API answered in the browser as the other
// mocked specs do. What the browser adds over the vitest suite: the redirect of an old Access
// link with its query string, the tabs at their own addresses, a tab's form as a page of its own,
// and a project role whose kind list holds project kinds only.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "eva.steward",
  name: "Eva Steward",
  email: "eva@hel.fi",
  roles: ["portal-editor"],
};

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const STEWARD = {
  project: "helsinki",
  bootstrap: false,
  grants: [
    {
      role: "steward",
      binding: "helsinki-stewards",
      scope: "project:helsinki",
      rule: {
        kinds: ["Pipeline", "Endpoint", "Role", "RoleBinding", "ServiceAccount", "Project"],
        verbs: ["propose", "approve", "delete"],
      },
    },
  ],
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path}`);
      return json({}, 202);
    }
    if (path.endsWith("/permissions/me")) return json(STEWARD);
    if (path === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (path === "/api/v1/projects/org/projects/helsinki") {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Project",
        metadata: { name: "helsinki", namespace: "org", title: "Helsinki city data" },
        spec: { organizationRef: "hel" },
      });
    }
    if (path === "/api/v1/projects/org/rolebindings") {
      return json(
        LIST([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "RoleBinding",
            metadata: { name: "stewards", namespace: "org" },
            spec: { subjects: [{ user: "eva@hel.fi" }], role: "steward", scope: { project: "helsinki" } },
          },
        ]),
      );
    }
    return json(LIST([]));
  });
  return { writes };
}

test.describe("Project settings", () => {
  test("an old Access link lands on Members with its query string, and the tabs have their own addresses", async ({
    page,
  }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/access?lang=en");
    await expect(page).toHaveURL(/\/projects\/helsinki\/settings\/members\?lang=en$/);
    await expect(page.getByRole("heading", { level: 1, name: "Project settings" })).toBeVisible();
    await expect(page.getByText("eva@hel.fi")).toBeVisible();

    await page.getByRole("tab", { name: "General" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/settings\/general$/);
    await expect(page.getByText("Helsinki city data")).toBeVisible();
  });

  test("a new role of the project offers project kinds only", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/settings/roles?lang=en");
    // the header action and, on an empty list, the empty state's own: either opens the form
    await page.getByRole("button", { name: "New role" }).first().click();
    const form = page.getByTestId("form-page").or(page.getByRole("dialog")).first();
    // Kinds are checkboxes (T-4213).
    await expect(form.getByRole("checkbox", { name: "Pipeline", exact: true }).first()).toBeAttached();
    for (const kind of ["Role", "RoleBinding", "Project"]) {
      await expect(form.getByRole("checkbox", { name: kind, exact: true })).toHaveCount(0);
    }
  });

  test("a tab's create form is a page of its own", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/settings/service-accounts/new?lang=en");
    await expect(page.getByRole("heading", { name: "New service account" })).toBeVisible();
  });

  test("has no axe violations", async ({ page }) => {
    await stubApi(page);
    await page.goto("/projects/helsinki/settings/members?lang=en");
    await expect(page.getByText("eva@hel.fi")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
