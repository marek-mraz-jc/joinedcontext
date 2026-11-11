import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6791: the Dashboard JSON editor in a real browser (Monaco and its JSON worker, the preview's
// Vega-Lite canvas) at desktop and phone width, each state shot to test-results/T-6791 for the
// task: the starter, a chart inserted by the picker, broken JSON named by line, and a widget the
// server refuses named by its JSON path.

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-viewer"] };
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "bbsk" },
  spec: { contextSpaceRef: "mobility", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] },
};
const SPACE = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "bbsk" }, spec: { dataModelRef: "bikes" } };
const MODEL = { apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "bikes", namespace: "bbsk" }, spec: { version: "1.0.0", classes: ["BikeHireDockingStation", "BikeLane"] } };
const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  type: "object",
  required: ["apiVersion", "kind", "metadata", "spec"],
  properties: {
    apiVersion: { type: "string" },
    kind: { type: "string" },
    metadata: { type: "object" },
    spec: { $ref: "#/definitions/DashboardSpec" },
  },
  definitions: {
    DashboardSpec: {
      type: "object",
      required: ["title", "pages"],
      properties: {
        title: { type: "string" },
        visibility: { type: "string", enum: ["private", "project", "organization", "public"] },
        pages: { type: "array", items: { $ref: "#/definitions/Page" } },
      },
    },
    Page: { type: "object", properties: { title: { type: "string" }, widgets: { type: "array", items: { $ref: "#/definitions/Widget" } } } },
    Widget: {
      type: "object",
      required: ["widgetType"],
      properties: { widgetType: { type: "string" }, endpointRef: { type: ["string", "null"] }, entityType: { type: "string" }, data: { type: "object" }, spec: { type: "object" } },
    },
  },
};
const CSV = "id,type,name\nurn:ngsi-ld:BikeHireDockingStation:1,BikeHireDockingStation,Kamppi\nurn:ngsi-ld:BikeHireDockingStation:2,BikeHireDockingStation,Töölö\nurn:ngsi-ld:BikeHireDockingStation:3,BikeHireDockingStation,Rautatientori\n";

async function stub(page: Page, refuse: boolean, endpoints: unknown[] = [ENDPOINT]) {
  await page.route("**/api/endpoint/**", (route) => route.fulfill({ contentType: "text/csv", body: CSV }));
  await page.route("**/api/v1/**", (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (request.method() !== "GET") {
      if (refuse && url.searchParams.get("dryRun") === "All") {
        return json(
          {
            title: "Invalid",
            status: 422,
            detail: "the dashboard reads an endpoint it may not",
            errors: [{ path: "/spec/pages/0/widgets/0/endpointRef", message: "names no endpoint of the project: parking" }],
          },
          422,
        );
      }
      return json({ valid: true, verdict: { ok: true, findings: [] } });
    }
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json(list([{ name: "bbsk" }]));
    if (url.pathname.endsWith("/kinds/Dashboard/schema")) return json(SCHEMA);
    if (url.pathname.endsWith("/endpoints")) return json(list(endpoints));
    if (url.pathname.endsWith("/spaces")) return json(list([SPACE]));
    if (url.pathname.endsWith("/datamodels")) return json(list([MODEL]));
    return json(list([]));
  });
}

/** Opens a phone tab and waits until it is drawn as the selected one (its colour transition). */
async function openTab(page: Page, name: string) {
  const tab = page.getByRole("tab", { name });
  await tab.click();
  await page.mouse.move(0, 0);
  await expect(tab).toHaveAttribute("aria-selected", "true");
  await expect(tab).not.toHaveCSS("border-bottom-color", "rgba(0, 0, 0, 0)");
}

for (const width of [1440, 400]) {
  test(`the dashboard JSON editor, every state, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false);
    await page.goto("/projects/bbsk/dashboard-editor?lang=en");
    await expect(page.getByRole("heading", { name: "New dashboard in JSON" })).toBeVisible();
    const editor = page.locator(".monaco-editor").first();
    await expect(editor).toBeVisible();
    await expect(page.getByText(width < 640 ? /draws nothing yet/ : /No widget yet/)).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `test-results/T-6791/editor-starter-${width}.png`, fullPage: true });
    expect(await axeViolations(page)).toEqual([]);

    if (width < 640) await page.getByText("Insert a widget", { exact: true }).click();
    await page.getByRole("button", { name: "Insert widget" }).click();
    // On a phone the preview is its own tab.
    if (width < 640) {
      await openTab(page, "Preview");
      // The check and Propose stay at hand on the preview tab.
      await expect(page.getByRole("button", { name: "Check" })).toBeVisible();
    }
    const figure = page.getByRole("figure").filter({ hasText: "BikeHireDockingStation by type" });
    await expect(figure.locator("canvas")).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `test-results/T-6791/editor-valid-${width}.png`, fullPage: true });

    // Break the JSON: the alert names the line, the preview waits.
    if (width < 640) await openTab(page, "Manifest (JSON)");
    await editor.click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type(",");
    await expect(page.getByText("The JSON does not read.")).toBeVisible();
    await expect(page.getByText(/^Line \d+, column \d+/)).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `test-results/T-6791/editor-invalid-${width}.png`, fullPage: true });
    expect(await axeViolations(page)).toEqual([]);
  });

  test(`a project with no Endpoint says where to make one, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false, []);
    await page.goto("/projects/bbsk/dashboard-editor?lang=en");
    await expect(page.getByText("No endpoint to read yet")).toBeVisible();
    await expect(page.getByRole("link", { name: "Go to endpoints" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Insert widget" })).toHaveCount(0);
    await page.screenshot({ path: `test-results/T-6791/editor-empty-${width}.png`, fullPage: true });
    expect(await axeViolations(page)).toEqual([]);
  });

  test(`a refusal names its JSON path, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, true);
    await page.goto("/projects/bbsk/dashboard-editor?lang=en");
    if (width < 640) await page.getByText("Insert a widget", { exact: true }).click();
    await page.getByRole("button", { name: "Insert widget" }).click();
    await page.getByRole("button", { name: "Check" }).click();
    await expect(page.getByText("/spec/pages/0/widgets/0/endpointRef", { exact: false }).first()).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 0));
    if (width < 640) {
      await page.screenshot({ path: `test-results/T-6791/editor-refused-list-${width}.png`, fullPage: true });
      await openTab(page, "Preview (fix the manifest)");
    }
    // The refused widget is not drawn: the preview shows what the server would publish.
    await expect(page.getByText("This widget is not drawn")).toBeVisible();
    await expect(page.getByRole("figure")).toHaveCount(0);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `test-results/T-6791/editor-refused-${width}.png`, fullPage: true });
    if (width < 640) {
      // The fix is in the manifest: one tap back from the refused widget.
      await page.getByRole("button", { name: "Show the manifest" }).click();
      await expect(page.locator(".monaco-editor").first()).toBeVisible();
    }
  });
}
