import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";

/** The project as the home reads it: `uploaded` holds rows and the private Endpoint the upload proposed. */
async function mock(page: Page, uploaded: boolean): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) => json({ apiVersion: API, kind: "List", items });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "e1", username: "eva", name: "Eva Editor", email: "eva@example.org", roles: [] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (url.pathname.endsWith("/permissions/me")) {
      return json({ project: "helsinki", grants: [{ rule: { kinds: ["ContextSpace", "DataSource", "Pipeline", "Endpoint"], verbs: ["read", "propose"] } }] });
    }
    if (url.pathname === "/api/v1/preferences") return json({});
    if (uploaded && url.pathname === "/api/v1/projects/helsinki/spaces") return list([{ metadata: { name: "parking" }, spec: {} }]);
    if (uploaded && url.pathname.endsWith("/spaces/parking/usage")) return json({ entities: 42, observedAt: "2026-10-21T20:00:00Z" });
    if (uploaded && url.pathname === "/api/v1/projects/helsinki/endpoints") {
      return list([{ metadata: { name: "parking" }, spec: { audience: "organization" } }]);
    }
    return list([]);
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

// The project home (T-3233, T-5109): a new editor signs in to a project that holds nothing yet,
// lands on its home, and meets the first-dataset path: upload a CSV, a private Endpoint, public on choice.
test("a new editor lands on the home and meets the three steps to a first dataset", async ({ page }) => {
  await mock(page, false);
  await page.goto("/?lang=en");
  await expect(page).toHaveURL(/\/projects\/helsinki\/home/);
  await expect(page.getByRole("heading", { level: 1, name: "Home" })).toBeVisible();
  const checklist = page.getByRole("region", { name: "Your first dataset" });
  await expect(checklist.getByRole("listitem")).toHaveCount(3);
  await expect(checklist.getByRole("link", { name: "Upload a CSV" })).toHaveAttribute("href", "/projects/helsinki/spaces/upload");
  await expect(checklist.getByRole("link", { name: "Or connect a live source by URL or API" })).toHaveAttribute("href", "/projects/helsinki/datasources/new");
  expect(await axeViolations(page)).toEqual([]);
  await checklist.getByRole("link", { name: "Upload a CSV" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
  await page.goto("/projects/helsinki/home?lang=en");
  await expect(checklist.getByRole("status")).toHaveText("0 of 3 done");
});

test("once the rows are in, the next step is the private endpoint's page, where the person may publish", async ({ page }) => {
  await mock(page, true);
  await page.goto("/projects/helsinki/home?lang=en");
  const checklist = page.getByRole("region", { name: "Your first dataset" });
  await expect(checklist.getByRole("status")).toHaveText("2 of 3 done");
  await expect(checklist.getByRole("link", { name: "Open the endpoint" })).toHaveAttribute("href", "/projects/helsinki/endpoints/parking");
  expect(await axeViolations(page)).toEqual([]);
});

for (const width of [1440, 400]) {
  for (const [lang, scheme] of [["en", "light"], ["sk", "dark"]] as const) {
    for (const uploaded of [false, true]) {
      test(`the checklist at ${width} px, ${lang}, ${scheme}, ${uploaded ? "uploaded" : "empty"}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.emulateMedia({ colorScheme: scheme });
        await mock(page, uploaded);
        await page.goto(`/projects/helsinki/home?lang=${lang}`);
        const checklist = page.getByRole("region", { name: lang === "en" ? "Your first dataset" : "Váš prvý dataset" });
        await expect(checklist.getByRole("status")).toContainText(uploaded ? "2" : "0");
        // Nothing wider than the screen: the steps and their hints wrap at 400 px.
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
        await shot(page, `home-${uploaded ? "uploaded" : "empty"}-${lang}-${scheme}-${width}`);
      });
    }
  }
}
