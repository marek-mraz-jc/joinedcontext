import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// The Endpoint form's datasets of one Endpoint (ADR-N-068, EP-174, EP-175), against a stubbed API
// as representation_outputs.spec.ts does. Shots of every state at 1440 and 400 go to
// test-results/catalog-datasets.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Example", email: "jana@example.org", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://example.org/models/stops",
  "name: stops",
  "classes:",
  "  Stop:",
  "    slots: [name, mode]",
  "  Platform:",
  "    slots: [name]",
  "slots:",
  "  name: { required: true }",
  "  mode: { range: string }",
  "",
].join("\n");
// What jc-manifest answers for a name another Endpoint publishes on the same catalogue (EP-177).
const REFUSAL =
  "Endpoint city/stops: not published: the CKAN dataset `stops-bus` on opendata is also what Endpoint city/buses publishes, and one catalogue holds one dataset of a name, so neither is written; give the spec.catalog.datasets entry a name of its own, such as `city-stops-bus` (EP-177)";

async function stubApi(page: Page): Promise<string[]> {
  const posted: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "city" }] });
    if (path.endsWith("/datamodels"))
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "stops", namespace: "city" }, spec: { contextSpaceRef: "transport", version: "1.0.0", lifecycle: "published", linkml: LINKML } }]));
    if (path.endsWith("/spaces")) return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "transport", namespace: "city" }, spec: {} }]));
    if (path.endsWith("/endpoints") && route.request().method() === "POST") {
      posted.push(route.request().postData() ?? "");
      return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: REFUSAL, errors: [REFUSAL] }, 422);
    }
    return json(list([]));
  });
  return posted;
}

const SHOTS = join(process.cwd(), "test-results", "catalog-datasets");

async function shot(page: Page, name: string, locator: ReturnType<Page["locator"]>): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  const width = page.viewportSize()?.width ?? 0;
  // The whole page at its full height, clipped to the section: the sticky header stays at the top
  // instead of covering the section mid-way, as a scrolled element shot draws it.
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height });
  const box = await locator.boundingBox();
  if (box) await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`), clip: box });
  await page.setViewportSize({ width, height: 900 });
}

for (const width of [1440, 400]) {
  test.describe(`datasets of one endpoint at ${width}`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("empty, two datasets filled with types from the model, a name the form refuses, then the server's 422", async ({ page }) => {
      const posted = await stubApi(page);
      await page.goto("/projects/city/endpoints/new?lang=en");
      await page.getByRole("textbox", { name: "Name", exact: true }).fill("stops");
      await page.getByRole("checkbox", { name: "Stop" }).check();
      await page.getByRole("checkbox", { name: "Platform" }).check();
      const group = page.locator("details").filter({ hasText: "Catalogue description" });
      await group.locator("summary").click();
      const add = page.locator("#root_catalog_datasets__add");
      const datasets = group.locator("fieldset", { has: add }).last();
      await expect(group.getByRole("checkbox", { name: "List it in the catalogue" })).toBeChecked();
      await shot(page, "1-empty", datasets);

      await add.click();
      await add.click();
      const names = datasets.getByRole("textbox", { name: "Dataset name" });
      await names.nth(0).fill("stops-tram");
      await datasets.getByRole("textbox", { name: "Title", exact: true }).nth(0).fill("Tram stops");
      await datasets.getByRole("textbox", { name: "Description", exact: true }).nth(0).fill("Every tram stop and its platforms.");
      // Each entry offers the model's two classes, in the model's order.
      const type = (name: string, entry: number) => datasets.getByRole("checkbox", { name, exact: true }).nth(entry);
      await expect(datasets.getByRole("checkbox", { name: "Stop", exact: true })).toHaveCount(2);
      await expect(datasets.getByRole("checkbox", { name: "Platform", exact: true })).toHaveCount(2);
      await type("Stop", 0).check();
      await type("Platform", 0).check();
      await names.nth(1).fill("stops-bus");
      await datasets.getByRole("textbox", { name: "Title", exact: true }).nth(1).fill("Bus stops");
      await datasets.getByRole("textbox", { name: "Description", exact: true }).nth(1).fill("Bus stops only.");
      await type("Stop", 1).check();
      await datasets.getByRole("textbox", { name: "Narrowing query" }).nth(1).fill('mode=="bus"');
      await expect(datasets.getByText("Value does not match pattern")).toHaveCount(0);
      await shot(page, "2-filled", datasets);

      await names.nth(1).fill("Bus Stops");

      await expect(datasets.getByText("Value does not match pattern")).toBeVisible();
      await shot(page, "3-invalid-name", datasets);

      await names.nth(1).fill("stops-bus");
      await expect(datasets.getByText("Value does not match pattern")).toHaveCount(0);
      posted.length = 0;
      await page.getByRole("button", { name: "Check", exact: true }).click();
      const refusal = page.getByRole("alert").filter({ hasText: "city-stops-bus" });
      await expect(refusal).toBeVisible();
      const body = JSON.parse(posted.at(-1) ?? "{}") as { spec?: { catalog?: { datasets?: unknown[] } } };
      expect(body.spec?.catalog?.datasets).toEqual([
        { name: "stops-tram", title: { en: "Tram stops" }, description: { en: "Every tram stop and its platforms." }, types: ["Stop", "Platform"] },
        { name: "stops-bus", title: { en: "Bus stops" }, description: { en: "Bus stops only." }, types: ["Stop"], q: 'mode=="bus"' },
      ]);
      await shot(page, "4-refused", refusal.locator("xpath=.."));
    });
  });
}
