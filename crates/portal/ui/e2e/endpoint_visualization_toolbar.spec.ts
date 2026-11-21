import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page, ["portal_visualization_template"]);
});

// T-7491: an editor gives a temporal chart the reader's toolbar defaults (range, granularity,
// method) and can hide the toolbar; the proposal carries them, a refusal names the member, and a
// reader without the propose right sees the charts only.

const API = "joinedcontext.com/v1alpha1";
const LINKML =
  "id: x\nname: x\nclasses:\n  AirQualityObserved:\n    slots: [name, dateObserved, no2]\nslots:\n  name: { required: true }\n  dateObserved: { range: datetime }\n  no2: { range: float }\n";
const LINE = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  mark: "line",
  encoding: { x: { field: "dateObserved", type: "temporal" }, y: { field: "no2", type: "quantitative" } },
};
const CHART = {
  name: "no2-week",
  title: { en: "NO2 this week" },
  description: { en: "Daily peak." },
  data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "P7D", aggrMethod: "max", aggrPeriod: "P1D", controls: false },
  spec: LINE,
};

function endpoint(visualizations: unknown[]) {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name: "air", namespace: "ovzdusie", labels: { "joinedcontext.com/space": "ovzdusie" } },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "zt4qm7ge2xdv6ksb3ncf5arw2y",
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "csv"],
      ...(visualizations.length ? { catalog: { visualizations } } : {}),
    },
    status: { phase: "Live" },
  };
}
const MODEL = { apiVersion: API, kind: "DataModel", metadata: { name: "air", namespace: "ovzdusie" }, spec: { contextSpaceRef: "ovzdusie", linkml: LINKML, version: "1.0.0" } };

interface Stub {
  verbs: string[];
  charts: unknown[];
  refuse?: boolean;
  puts: { url: URL; body: Record<string, unknown> }[];
}

async function stub(page: Page, s: Stub) {
  const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "editor", name: "Editor", email: "e@example.org", roles: ["portal-editor"] });
    if (path.endsWith("/permissions/me"))
      return json({ project: "ovzdusie", grants: [{ role: "steward", rule: { verbs: s.verbs, kinds: ["Endpoint", "DataModel", "ContextSpace"] }, binding: "stewards", scope: "project:ovzdusie" }] });
    if (path === "/api/v1/projects") return json(list([{ name: "ovzdusie" }]));
    if (path.startsWith("/api/v1/visualization-templates/")) return json(LINE);
    if (path === "/api/v1/projects/ovzdusie/endpoints/air" && request.method() === "PUT") {
      s.puts.push({ url, body: JSON.parse(request.postData() ?? "{}") });
      if (s.refuse)
        return json(
          {
            type: "about:blank",
            title: "The manifest is not valid",
            status: 422,
            detail: "/spec/catalog/visualizations/0/data/last: source: `P32D` is not an ISO 8601 duration up to P31D",
            errors: [{ field: "/spec/catalog/visualizations/0/data/last", reason: "source: `P32D` is not an ISO 8601 duration up to P31D" }],
          },
          422,
        );
      if (url.searchParams.get("dryRun")) return json({ valid: true });
      return json({ metadata: { name: "chg-1" }, status: { phase: "Proposed" } }, 202);
    }
    if (path === "/api/v1/projects/ovzdusie/endpoints/air") return json(endpoint(s.charts));
    if (path === "/api/v1/projects/ovzdusie/endpoints") return json(list([endpoint(s.charts)]));
    if (path === "/api/v1/projects/ovzdusie/datamodels") return json(list([MODEL]));
    return json(list([]));
  });
  await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
  await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));
}

const DIR = join("test-results", "T-7491");
mkdirSync(DIR, { recursive: true });

for (const width of [1440, 400]) {
  test(`an editor sets the reader's toolbar defaults of a temporal chart and proposes it at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const s: Stub = { verbs: ["read", "propose", "approve"], charts: [], puts: [] };
    await stub(page, s);
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByText("Add a chart").click();
    const toolbar = section.getByTestId("viz-temporal");
    await expect(toolbar).toBeVisible();
    await toolbar.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-empty-${width}.png`) });

    // Raw readings take no method.
    await page.getByLabel("Default granularity").selectOption("raw");
    await expect(page.getByLabel("Default method")).toHaveCount(0);
    await page.getByLabel("Default granularity").selectOption("daily");
    await page.getByLabel("Default range").selectOption("week");
    await page.getByLabel("Default method").selectOption("max");
    await page.getByLabel("Show the toolbar to readers").uncheck();
    await expect(page.getByLabel("Default range")).toHaveAccessibleDescription(/e\.g\. 7 days/);

    await page.getByLabel("Horizontal axis").selectOption("dateObserved");
    await page.getByLabel("Measure").selectOption("no2");
    await page.getByRole("button", { name: "Fill from the chart" }).click();
    await expect(page.getByLabel("Vega-Lite specification")).toHaveValue(/dateObserved/);
    await page.getByLabel("Name", { exact: true }).fill("no2-week");
    await page.getByLabel("Title", { exact: true }).fill("NO2 this week");
    await page.getByLabel("Description", { exact: true }).fill("Daily peak.");
    await toolbar.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-filled-${width}.png`) });

    await page.getByRole("button", { name: "Propose the chart" }).click();
    await expect.poll(() => s.puts.length).toBe(2);
    expect(s.puts[1].body).toMatchObject({ spec: { catalog: { visualizations: [CHART] } } });
    await expect(page.getByText("chg-1").first()).toBeVisible();
    await page.screenshot({ path: join(DIR, `toolbar-proposed-${width}.png`) });
  });

  test(`a refused default names the member at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const s: Stub = { verbs: ["read", "propose", "approve"], charts: [], puts: [], refuse: true };
    await stub(page, s);
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByText("Add a chart").click();
    await page.getByLabel("Horizontal axis").selectOption("dateObserved");
    await page.getByLabel("Measure").selectOption("no2");
    await page.getByRole("button", { name: "Fill from the chart" }).click();
    await expect(page.getByLabel("Vega-Lite specification")).toHaveValue(/dateObserved/);
    await page.getByLabel("Name", { exact: true }).fill("no2-week");
    await page.getByLabel("Title", { exact: true }).fill("NO2 this week");
    await page.getByLabel("Description", { exact: true }).fill("Daily peak.");
    await page.getByRole("button", { name: "Propose the chart" }).click();
    const alert = section.getByRole("alert");
    await expect(alert).toContainText("visualizations/0/data/last");
    await alert.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-refused-${width}.png`) });
  });

  test(`a reader without the propose right sees the charts and no editor at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await stub(page, { verbs: ["read"], charts: [CHART], puts: [] });
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await expect(section.getByText("NO2 this week")).toBeVisible();
    await expect(section.getByText("Add a chart")).toHaveCount(0);
    await section.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-reader-${width}.png`) });
  });
}
