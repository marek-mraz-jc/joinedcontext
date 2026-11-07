import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Where a first sign-in lands and what a project switch reopens (T-5603). `vite preview` has no
// Portal API behind it, so the API is answered in the browser; the pages are the real build.

const IDENTITY = { subject: "v1e2", username: "demo.viewer", name: "Demo Viewer", email: "demo.viewer@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
// The sample is listed first by name, as on dev.
const PROJECTS = list([
  { name: "banskabystrica", sample: true, title: "Banská Bystrica" },
  { name: "helsinki", sample: false, title: "Helsinki" },
]);
const ROWS = "/projects/helsinki/spaces/helsinki?type=AirQualityObserved";

async function stubApi(page: Page): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json(PROJECTS);
    if (path.endsWith("/spaces/helsinki/views")) return json({ items: [] });
    if (path.endsWith("/spaces/helsinki")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ContextSpace",
        metadata: { name: "helsinki", namespace: "helsinki", title: { en: "Helsinki city context" } },
        spec: {},
        status: { phase: "Live" },
      });
    }
    if (path.endsWith("/entities")) return json([]);
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

/** Picks `project` in the project switcher, behind the menu button below 1024 px. */
async function switchTo(page: Page, width: number, project: RegExp): Promise<void> {
  if (width < 1024) {
    await page.getByRole("button", { name: "Menu" }).click();
  }
  await page.getByRole("button", { name: "Projects" }).click();
  await page.getByRole("menuitem", { name: project }).click();
}

test.describe("the project a person lands in", () => {
  for (const width of [1440, 400]) {
    test(`opens the own project first and reopens the space and type left there at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page);
      await page.goto("/?lang=en");
      await expect(page).toHaveURL(/\/projects\/helsinki\/home/);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await shot(page, `landing-first-sign-in-${width}`);

      await page.goto(ROWS);
      await switchTo(page, width, /Banská Bystrica/);
      await expect(page).toHaveURL(/\/projects\/banskabystrica\/spaces/);
      await switchTo(page, width, /Helsinki/);
      await expect(page).toHaveURL((url) => `${url.pathname}${url.search}` === ROWS);
      await expect(page.getByRole("heading", { name: "Helsinki city context" })).toBeVisible();
      await shot(page, `landing-switch-back-${width}`);
    });
  }
});
