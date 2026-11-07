import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Settings (T-6344): the account menu holds Settings, Administration for a platform administrator
// and Sign out; /settings lists the person's own, the project's and the platform's settings; the
// signed-in footer links the accessibility statement that left the menu. `vite preview` has no
// Portal API behind it, so the API is answered in the browser.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: ["RoleBinding", "Organization", "Group"].map((kind) => ({
    role: "platform-admin",
    binding: "platform-admins",
    scope: "organization",
    rule: { kinds: [kind], verbs: ["propose", "approve", "delete"] },
  })),
};

async function stubApi(page: Page, admin: boolean): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(list([{ name: "doprava", sample: false, title: "Doprava" }]));
    if (path === "/api/v1/projects/org/permissions/me") return json(admin ? ADMIN : { project: "org", bootstrap: false, grants: [] });
    if (path.endsWith("/permissions/me")) return json({ project: "doprava", bootstrap: false, grants: [] });
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("settings", () => {
  for (const width of [1440, 400]) {
    test(`the account menu opens Settings, which lists personal and project settings at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, false);
      await page.goto("/projects/doprava/spaces?lang=en");
      await expect(page.getByRole("link", { name: "Accessibility", exact: true })).toHaveAttribute("href", "/accessibility");
      await page.getByRole("button", { name: /Signed in as/ }).click();
      await expect(page.getByRole("menuitem", { name: "Administration" })).toHaveCount(0);
      await shot(page, `account-menu-${width}`);
      await page.getByRole("menuitem", { name: "Settings" }).click();
      await expect(page).toHaveURL(/\/settings/);
      await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
      await expect(page.getByRole("link", { name: "My AI connectors" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Settings of doprava" })).toBeVisible();
      await expect(page.getByText("Platform", { exact: true })).toHaveCount(0);
      await shot(page, `settings-${width}`);
    });

    test(`a platform administrator also finds Administration at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true);
      await page.goto("/settings?lang=en");
      await expect(page.getByText("Platform", { exact: true })).toBeVisible();
      await expect(page.getByRole("main").getByRole("link", { name: "Administration" })).toBeVisible();
      await shot(page, `settings-admin-${width}`);
      await page.getByRole("button", { name: /Signed in as/ }).click();
      await expect(page.getByRole("menuitem", { name: "Administration" })).toBeVisible();
      await shot(page, `account-menu-admin-${width}`);
    });
  }
});
