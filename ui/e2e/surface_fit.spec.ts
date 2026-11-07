import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The Endpoint form greys out a surface the space's data model cannot serve (EP-125, T-5299), from
// GET …/spaces/{space}/surface-fit, against a stubbed API as representation_outputs.spec.ts does.
// The Portal refuses the same surface on propose (tests/surface_fit_api_tests.rs). Shots of each
// state at 1440 and 400 go to test-results/surface-fit.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@example.org", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const STA_GAP = "`sta` needs a type with a numeric attribute that declares its unit";
const DRAFT = (r: string) => `\`${r}\` needs a published data model in space ovzdusie; its model is draft (EP-125, DM-26)`;
const FITS = {
  fit: { dataModel: "air", surfaces: [{ representation: "sta", gap: null }, { representation: "ical", gap: null }] },
  refused: { dataModel: "air", surfaces: [{ representation: "sta", gap: STA_GAP }, { representation: "ical", gap: null }] },
  draft: { dataModel: "air", surfaces: [{ representation: "sta", gap: DRAFT("sta") }, { representation: "ical", gap: DRAFT("ical") }] },
  none: { dataModel: null, surfaces: [{ representation: "sta", gap: STA_GAP }, { representation: "ical", gap: null }] },
};

async function stubApi(page: Page, fit: keyof typeof FITS): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "city" }] });
    if (path === "/api/v1/projects/city/spaces/ovzdusie/surface-fit") {
      asked.push(path);
      return json(FITS[fit]);
    }
    if (path.endsWith("/spaces"))
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "ovzdusie", namespace: "city" }, spec: {} }]));
    return json(list([]));
  });
  return asked;
}

const SHOTS = join(process.cwd(), "test-results", "surface-fit");

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${name}-${page.viewportSize()?.width ?? 0}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`surface fit at ${width}`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a model that serves SensorThings leaves its box free", async ({ page }) => {
      const asked = await stubApi(page, "fit");
      await page.goto("/projects/city/endpoints/new?lang=en");
      const sta = page.getByRole("checkbox", { name: "SensorThings API" });
      await expect(sta).toBeEnabled();
      await sta.check();
      await expect(page.getByTestId("surface-gaps")).toHaveCount(0);
      expect(asked.length).toBeGreaterThan(0);
      await shot(page, "1-fit");
    });

    test("a model without a unit greys SensorThings out, says why and links the model", async ({ page }) => {
      await stubApi(page, "refused");
      await page.goto("/projects/city/endpoints/new?lang=en");
      await expect(page.getByRole("checkbox", { name: "SensorThings API" })).toBeDisabled();
      await expect(page.getByRole("checkbox", { name: "iCalendar feed" })).toBeEnabled();
      await expect(page.getByTestId("surface-gaps")).toHaveText(STA_GAP);
      await expect(page.getByRole("link", { name: "Open the data model air" })).toHaveAttribute("href", "/projects/city/models/air");
      await shot(page, "2-refused");
    });

    test("a draft model backs no domain standard", async ({ page }) => {
      await stubApi(page, "draft");
      await page.goto("/projects/city/endpoints/new?lang=en");
      await expect(page.getByRole("checkbox", { name: "SensorThings API" })).toBeDisabled();
      await expect(page.getByRole("checkbox", { name: "iCalendar feed" })).toBeDisabled();
      await expect(page.getByTestId("surface-gaps").locator("li")).toHaveText([DRAFT("sta"), DRAFT("ical")]);
      await shot(page, "3-draft");
    });

    test("a space without a model points at adding one", async ({ page }) => {
      await stubApi(page, "none");
      await page.goto("/projects/city/endpoints/new?lang=en");
      await expect(page.getByRole("checkbox", { name: "SensorThings API" })).toBeDisabled();
      await expect(page.getByRole("link", { name: "Add a data model for the space" })).toHaveAttribute("href", "/projects/city/models");
      await shot(page, "4-no-model");
    });
  });
}
