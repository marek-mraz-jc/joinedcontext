import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// The sidebar head: the project switcher, its menu and New project (T-7148, owner screenshot
// 2026-11-16 where every name was cut). `vite preview` has no Portal API behind it, so the API
// is answered in the browser; the pages are the real build.

const IDENTITY = { subject: "v1e2", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ONE = [{ name: "banskabystrica" }];
const FIVE = [
  { name: "banskabystrica-mestsky-urad-odbor-dopravy", title: "Mestský úrad Banská Bystrica, odbor dopravy a životného prostredia" },
  { name: "banskabystrica-mestske-lesy-a-parky-sprava", title: "Mestské lesy Banská Bystrica, správa parkov a zelene" },
  { name: "helsinki-kaupunkiymparisto-ilmanlaatu-2026" },
  { name: "zvolen-tepelne-hospodarstvo-merania", title: "Zvolen: tepelné hospodárstvo a merania spotreby" },
  { name: "brezno" },
];
const CATALOGUE = { total: 0, page: 1, pageSize: 20, datasets: [], facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] }, unavailable: [] };
const REFUSED = "Only an organization administrator opens a project here.";

async function stubApi(page: Page, projects: unknown[], refuse = false): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(list(projects));
    // With no project the Portal opens the catalogue of what the person may read (T-6376).
    if (path === "/api/v1/catalogue") return json(CATALOGUE);
    if (path === "/api/v1/catalogue/readable") return json({ datasets: [], unanswered: 0 });
    if (path.endsWith("/permissions/me")) {
      return json({ grants: [], projects: { creation: refuse ? { allowed: false, reason: REFUSED } : { allowed: true } } });
    }
    return json(list([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them (`JC_SHOT_PREFIX` before/after). */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${process.env.JC_SHOT_PREFIX ?? "after"}-${name}.png`) });
}

async function openSidebar(page: Page, width: number): Promise<void> {
  if (width < 1024) await page.getByRole("button", { name: "Menu" }).click();
  await expect(page.locator("#portal-sidebar")).toBeVisible();
}

test.describe("the sidebar head", () => {
  for (const width of [1440, 400]) {
    test(`one untitled project at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, ONE);
      await page.goto("/projects/banskabystrica/spaces?lang=en");
      await openSidebar(page, width);
      await expect(page.getByRole("button", { name: "Projects" })).toHaveAttribute("title", "banskabystrica");
      await shot(page, `one-${width}`);
      await page.getByRole("button", { name: "Projects" }).click();
      await expect(page.getByRole("menuitem", { name: /banskabystrica/ })).toBeVisible();
      await shot(page, `one-menu-${width}`);
    });

    test(`five long names, titled and untitled, menu open at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, FIVE);
      const active = FIVE[0] as { name: string; title: string };
      await page.goto(`/projects/${active.name}/spaces?lang=en`);
      await openSidebar(page, width);
      await expect(page.getByRole("button", { name: "Projects" })).toHaveAttribute("title", `${active.title} (${active.name})`);
      await shot(page, `five-${width}`);
      await page.getByRole("button", { name: "Projects" }).click();
      const menu = page.getByRole("menu");
      for (const project of FIVE) {
        await expect(menu.getByText((project as { title?: string }).title ?? project.name, { exact: true })).toBeVisible();
      }
      // The menu fits the screen: nothing of it hangs past the right edge.
      const box = await menu.boundingBox();
      expect(box && box.x + box.width).toBeLessThanOrEqual(width);
      await shot(page, `five-menu-${width}`);
      await menu.getByRole("menuitem", { name: /Zvolen/ }).click();
      await expect(page).toHaveURL(/\/projects\/zvolen-tepelne-hospodarstvo-merania\//);
    });

    // New project is the switcher's last item, after a divider, never a line of its own (T-7404).
    test(`New project in the switcher opens its dialog at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, FIVE.slice(2, 4));
      await page.goto("/projects/zvolen-tepelne-hospodarstvo-merania/spaces?lang=en");
      await openSidebar(page, width);
      await expect(page.locator("#portal-sidebar").getByRole("button", { name: "New project" })).toHaveCount(0);
      await page.getByRole("button", { name: "Projects" }).click();
      const item = page.getByRole("menu").getByRole("menuitem", { name: "New project" });
      await expect(item).toBeVisible();
      await shot(page, `new-in-menu-${width}`);
      await item.click();
      await expect(page.getByRole("dialog", { name: "Open a project" })).toBeVisible();
      await shot(page, `new-dialog-${width}`);
    });

    test(`New project refused with its reason at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, FIVE.slice(2, 4), true);
      await page.goto("/projects/zvolen-tepelne-hospodarstvo-merania/spaces?lang=en");
      await openSidebar(page, width);
      await page.getByRole("button", { name: "Projects" }).click();
      const item = page.getByRole("menu").getByRole("menuitem", { name: /New project/ });
      await expect(item).toHaveAttribute("aria-disabled", "true");
      await expect(item).toContainText(REFUSED);
      await shot(page, `refused-${width}`);
    });

    test(`no project at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, []);
      await page.goto("/?lang=en");
      await expect(page).toHaveURL(/\/catalogue/);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(page.getByText("This page stopped")).toHaveCount(0);
      await shot(page, `empty-${width}`);
    });
  }
});
