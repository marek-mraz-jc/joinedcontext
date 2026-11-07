import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// An administrator of an Organization other than the installation's (ADR-N-058 section 2.4,
// T-5248) opens Administration and finds that Organization's members, groups and settings, and
// nothing of the installation's. `vite preview` has no Portal API behind it, so the API is
// answered in the browser as the server answers this person: Žilina's manifests alone.

const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: object) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "org" },
  spec,
});
const PROJECTS = {
  ...list([
    { name: "helsinki", sample: false, organization: "hel", title: "Helsinki" },
    { name: "ovzdusie", sample: false, organization: "zilina", title: "Ovzdušie Žilina" },
  ]),
  organizations: [
    { name: "hel", title: "City of Helsinki" },
    { name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } },
  ],
};
const PERMISSIONS = {
  project: "org",
  bootstrap: false,
  grants: ["RoleBinding", "Organization", "Group"].map((kind) => ({
    role: "org-admin",
    binding: "zilina-admins",
    scope: "organization",
    organization: "zilina",
    rule: { kinds: [kind], verbs: ["propose", "approve"] },
  })),
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(PROJECTS);
    if (path === "/api/v1/projects/org/permissions/me") return json(PERMISSIONS);
    if (path === "/api/v1/projects/org/rolebindings") {
      return json(
        list([
          manifest("RoleBinding", "zilina-admins", {
            role: "org-admin",
            subjects: [{ user: "eva@zilina.sk" }],
            scope: { organization: "zilina" },
          }),
        ]),
      );
    }
    if (path === "/api/v1/projects/org/organizations") {
      return json(list([manifest("Organization", "zilina", { domain: "zilina.sk", locales: ["sk", "en"], defaultLocale: "sk" })]));
    }
    return json(list([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("an Organization's own administrator", () => {
  for (const width of [1440, 400]) {
    test(`opens Žilina's members and settings and no installation section at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      await page.goto("/projects/ovzdusie/spaces?lang=en");
      // On a phone Administration sits in the account menu (T-3319).
      if (width < 640) {
        await page.getByRole("button", { name: /Signed in as/ }).click();
        await page.getByRole("menuitem", { name: "Administration" }).click();
      } else {
        await page.getByRole("link", { name: "Administration" }).click();
      }
      await expect(page).toHaveURL(/\/organization\/settings/);
      await expect(page.getByText("zilina.sk").first()).toBeVisible();
      await shot(page, `own-organization-settings-${width}`);

      await page.goto("/organization/members?lang=en");
      await expect(page.getByText("eva@zilina.sk").first()).toBeVisible();
      if (width < 1024) {
        await page.getByRole("button", { name: "Menu" }).click();
      }
      const nav = page.getByRole("list", { name: "People and access" }).last();
      await expect(nav.getByRole("link")).toHaveText(["Members", "Groups"]);
      await expect(page.getByRole("link", { name: "Brokers" })).toHaveCount(0);
      await shot(page, `own-organization-members-${width}`);

      await page.goto("/organization/brokers?lang=en");
      await expect(page.getByText("This page is for organization administrators.")).toBeVisible();
      await shot(page, `own-organization-refused-${width}`);
    });
  }
});
