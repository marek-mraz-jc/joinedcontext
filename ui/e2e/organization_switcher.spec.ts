import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The organization switcher as a person in two Organizations meets it (PF-124, ADR-N-058 section
// 2.7). `vite preview` has no Portal API behind it, so the API is answered in the browser; the
// pages are the real build.

const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const PROJECTS = {
  ...list([
    { name: "helsinki", sample: false, organization: "hel", title: "Helsinki" },
    { name: "espoo", sample: false, organization: "hel", title: "Espoo" },
    { name: "zilina", sample: false, organization: "zilina", title: "Mesto Žilina" },
  ]),
  organizations: [
    { name: "hel", title: "City of Helsinki" },
    { name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } },
  ],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(PROJECTS);
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

test.describe("the organization switcher", () => {
  for (const width of [1440, 400]) {
    test(`switches from Helsinki to Žilina and keeps the choice in ?org= at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      await page.goto("/projects/helsinki/spaces?lang=en");
      // On a phone the switcher is atop the sidebar, behind the menu button.
      if (width < 640) {
        await page.getByRole("button", { name: "Menu" }).click();
      }
      const switcher = page.getByRole("button", { name: "Switch organization, now City of Helsinki" });
      await expect(switcher).toBeVisible();
      await switcher.click();
      await expect(page.getByRole("menuitem")).toHaveCount(2);
      await shot(page, `organization-switcher-open-${width}`);
      await page.getByRole("menuitem", { name: /City of Žilina/ }).click();

      await expect(page).toHaveURL(/\/projects\/zilina\/spaces\?org=zilina/);
      await expect(page.getByRole("button", { name: "Switch organization, now City of Žilina" })).toBeVisible();
      if (width < 1024 && width >= 640) {
        await page.getByRole("button", { name: "Menu" }).click();
      }
      await page.getByRole("button", { name: "Projects" }).click();
      await expect(page.getByRole("menuitem")).toHaveCount(1);
      await expect(page.getByRole("menuitem")).toContainText("Mesto Žilina");
      await shot(page, `organization-switcher-zilina-projects-${width}`);
    });
  }
});
