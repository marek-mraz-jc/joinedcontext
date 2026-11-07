import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// Administration access forms in plain words (T-4832): a new Role and a new service account start
// from a preset (Reader, Writer, Steward, Pipeline writer) and show the raw kinds, verbs and CIM 009
// operations only under Custom. `vite preview` has no Portal API, so the API is answered here.

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const KINDS = ["ContextSpace", "DataSource", "Endpoint", "Entity", "Group", "Pipeline", "RoleBinding", "ServiceAccount", "Role"];

async function mock(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "steward", name: "Stela Steward", email: "steward@example.org", roles: [] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ rule: { kinds: KINDS, verbs: ["read", "propose", "approve", "delete"] } }] });
    }
    if (url.pathname === "/api/v1/preferences") return json({ firstRunDismissed: true });
    return json(LIST([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`access presets at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await mock(page);
    });

    test("a new role starts as a Reader and shows its rules only under Custom", async ({ page }) => {
      await page.goto("/organization/roles/new?lang=en");
      const reader = page.getByRole("radio", { name: "Reader" });
      await expect(reader).toBeChecked();
      await expect(page.getByRole("checkbox", { name: "Pipeline", exact: true })).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `role-preset-${width}`);
      await page.getByRole("radio", { name: "Custom" }).check();
      await expect(page.getByRole("heading", { name: "Advanced" })).toBeVisible();
      await expect(page.getByRole("checkbox", { name: "Pipeline", exact: true }).first()).toBeVisible();
      await shot(page, `role-custom-${width}`);
    });

    test("a new service account grant reads as a preset, its operations under Custom", async ({ page }) => {
      await page.goto("/organization/service-accounts/new?lang=en");
      await expect(page.getByRole("radio", { name: "Reader" })).toBeChecked();
      await expect(page.getByTestId("operations-picker")).toHaveCount(0);
      await page.getByRole("radio", { name: "Pipeline writer" }).check();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `service-account-preset-${width}`);
      await page.getByRole("radio", { name: "Custom" }).check();
      await expect(page.getByTestId("operations-picker")).toBeVisible();
      await shot(page, `service-account-custom-${width}`);
    });

    test("a group and a grant stay one short form", async ({ page }) => {
      await page.goto("/organization/groups/new?lang=en");
      await expect(page.getByRole("heading", { name: "New group" })).toBeVisible();
      await shot(page, `group-${width}`);
      await page.goto("/organization/members/new?lang=en");
      await expect(page.getByRole("heading", { name: "Grant a role" })).toBeVisible();
      await shot(page, `grant-${width}`);
    });
  });
}
