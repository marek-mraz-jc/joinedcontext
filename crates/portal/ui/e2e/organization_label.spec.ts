import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// No Portal lists Organizations (T-7151): on an Organization's own host the header names that
// Organization with its logo and offers no menu of others; on the deployment's own address an
// operator gets no list either. `vite preview` has no Portal API behind it, so the API is
// answered in the browser; the pages are the real build.

const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const BRANDING = { instanceName: "Mesto Žilina", shortName: "Žilina" };

/** The project list as the server answers it: `organizations` holds the host's own alone. */
function projects(host: { name: string; title: string } | undefined) {
  return {
    ...list([
      { name: "zilina", sample: false, organization: "zilina", title: "Mesto Žilina" },
      ...(host ? [] : [{ name: "helsinki", sample: false, organization: "hel", title: "Helsinki" }]),
    ]),
    organizations: host ? [host] : [],
  };
}

async function stubApi(page: Page, host: { name: string; title: string } | undefined): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(projects(host));
    if (path.endsWith("/branding") && host) return json(BRANDING);
    return json(list([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: false });
}

/** No control anywhere that names, switches or lists Organizations. */
async function expectNoOrganizationMenu(page: Page): Promise<void> {
  await expect(page.getByRole("button", { name: /organi[sz]ation/i })).toHaveCount(0);
  await expect(page.getByText(/Choose an organization|for example Mesto/i)).toHaveCount(0);
}

test.describe("the Organization in the header", () => {
  for (const width of [1440, 400]) {
    test(`an Organization's host names it with no menu at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { name: "zilina", title: "Mesto Žilina" });
      await page.goto("/projects/zilina/spaces?lang=en");
      await expect(page.getByRole("banner").getByText(/Žilina/).first()).toBeAttached();
      await expectNoOrganizationMenu(page);
      await shot(page, `header-organization-host-${width}`);
      // The sidebar (behind the menu button below lg) holds the project, no Organization.
      if (width < 1024) await page.getByRole("button", { name: "Menu" }).click();
      await expectNoOrganizationMenu(page);
      await page.getByRole("button", { name: "Projects" }).click();
      await expect(page.getByRole("menuitem")).toHaveCount(1);
      await shot(page, `header-organization-host-projects-${width}`);
    });

    test(`the deployment's own address lists no Organizations at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, undefined);
      await page.goto("/projects/zilina/spaces?lang=en&org=hel");
      await expectNoOrganizationMenu(page);
      if (width < 1024) await page.getByRole("button", { name: "Menu" }).click();
      await expectNoOrganizationMenu(page);
      await shot(page, `header-installation-${width}`);
    });
  }
});
