import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
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
  encoding: { x: { field: "observedAt", type: "temporal" }, y: { field: "no2", type: "quantitative" } },
};
const CHART = {
  name: "no2-week",
  title: { en: "NO2 this week" },
  description: { en: "Daily peak." },
  data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "P7D", aggrMethod: "max", aggrPeriod: "P1D", controls: false },
  spec: LINE,
};

const SHOWN = {
  ...CHART,
  name: "no2-day",
  title: { en: "NO2 today" },
  data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "PT24H", aggrMethod: "avg", aggrPeriod: "PT1H" },
};
const FIXED = {
  ...CHART,
  name: "no2-raw",
  title: { en: "NO2 readings" },
  data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "PT24H", controls: false },
};

// As the Portal words each refusal: judge's source check, and jc-core's spec check behind its prefix.
const OVER_31_DAYS = "invalid /spec/catalog/visualizations/0/data/last: source: `P90D` is not an ISO 8601 duration up to P31D";
const SPEC_READS_A_URL =
  "spec is not a valid Endpoint: invalid /spec/catalog/visualizations/0/spec/data/url: data-source: the rows come from the Endpoint's declared source, never a URL";

// Written by hand, before the check refused it: a range over 31 days.
const BROKEN = { ...CHART, name: "no2-quarter", title: { en: "NO2 this quarter" }, data: { ...CHART.data, last: "P90D" } };

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
  /** The sentence the check refuses with. */
  refuse?: string;
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
      if (s.refuse !== undefined) {
        // As the Portal's check answers: a verdict, the pointer in the sentence, no path.
        const message = s.refuse;
        return json({ valid: false, verdict: { ok: false, findings: [{ level: "error", path: "", message }] } });
      }
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

/** The whole form in one picture, in a window tall enough that nothing floats over it. */
async function shootForm(page: Page, file: string, width: number) {
  await page.setViewportSize({ width, height: 2600 });
  await page.getByTestId("visualizations").locator("details").first().screenshot({ path: join(DIR, file) });
  await page.setViewportSize({ width, height: 1000 });
}

const DIR = join("test-results", "T-7491");
mkdirSync(DIR, { recursive: true });

for (const width of [1440, 400]) {
  test.describe(`${width} px`, () => {
  // A phone has a finger: the controls grow to 44 px only under a coarse pointer.
  test.use(width === 400 ? { hasTouch: true, isMobile: true } : {});
  test(`an editor sets the reader's toolbar defaults of a temporal chart and proposes it at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const s: Stub = { verbs: ["read", "propose", "approve"], charts: [], puts: [] };
    await stub(page, s);
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByText("Add a chart").click();
    const toolbar = section.getByTestId("viz-temporal");
    await expect(toolbar).toBeVisible();
    // Every field says what it takes, with an example, at the size of the body text.
    for (const field of await section.locator("details input, details select, details textarea").all()) {
      await expect(field).toHaveAccessibleDescription(/e\.g\./);
    }
    const size = (locator: Locator) => locator.evaluate((node) => getComputedStyle(node).fontSize);
    expect(await size(section.locator("#viz-title__help"))).toBe(await size(section.locator("label[for=viz-title]")));
    await shootForm(page, `toolbar-empty-${width}.png`, width);

    // Raw readings take no method.
    await page.getByLabel("Default granularity").selectOption("raw");
    await expect(page.getByLabel("Default method")).toHaveCount(0);
    await toolbar.screenshot({ path: join(DIR, `toolbar-raw-${width}.png`) });
    await page.getByLabel("Default granularity").selectOption("daily");
    await page.getByLabel("Default range").selectOption("week");
    await page.getByLabel("Default method").selectOption("max");
    await expect(page.getByLabel("Default range")).toHaveAccessibleDescription(/e\.g\. 7 days for the last week/);
    await page.getByLabel("Show the toolbar to readers").uncheck();
    await expect(page.getByLabel("Range", { exact: true })).toHaveValue("week");
    await expect(page.getByLabel("Show the toolbar to readers")).toHaveAccessibleDescription(/always see the settings below/);

    await expect(page.getByLabel("Horizontal axis")).toHaveText("Time of the reading (observedAt)");
    await page.getByLabel("Measure").selectOption("no2");
    // Nobody has to open the JSON: proposing writes it.
    await expect(page.getByLabel("Chart specification (Vega-Lite)")).toBeHidden();
    await page.getByRole("textbox", { name: "Short ID", exact: true }).fill("no2-week");
    await page.getByRole("textbox", { name: "Title", exact: true }).fill("NO2 this week");
    await page.getByRole("textbox", { name: "Description", exact: true }).fill("Daily peak.");
    await shootForm(page, `toolbar-filled-${width}.png`, width);

    await page.getByRole("button", { name: "Propose the chart" }).click();
    await expect.poll(() => s.puts.length).toBe(2);
    expect(s.puts[1].body).toMatchObject({ spec: { catalog: { visualizations: [CHART] } } });
    await expect(page.getByText("chg-1").first()).toBeVisible();
    await expect(section.getByText("“NO2 this week” is waiting for approval")).toBeVisible();
    await section.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-proposed-${width}.png`) });
  });

  test(`a refused specification says what to change on its field at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const s: Stub = { verbs: ["read", "propose", "approve"], charts: [], puts: [], refuse: SPEC_READS_A_URL };
    await stub(page, s);
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByText("Add a chart").click();
    await page.getByLabel("Measure").selectOption("no2");
    await section.getByText("Advanced: edit the chart's specification").click();
    await page.getByRole("button", { name: "Write it from the fields above" }).click();
    const spec = page.getByLabel("Chart specification (Vega-Lite)");
    await expect(spec).toHaveValue(/observedAt/);
    // Edited by hand to read another server: the one refusal this form can get for a new chart.
    await spec.fill(JSON.stringify({ ...LINE, data: { url: "https://example.org/no2.csv" } }, null, 2));
    await page.getByRole("textbox", { name: "Short ID", exact: true }).fill("no2-week");
    await page.getByRole("textbox", { name: "Title", exact: true }).fill("NO2 this week");
    await page.getByRole("textbox", { name: "Description", exact: true }).fill("Daily peak.");
    await page.getByRole("button", { name: "Propose the chart" }).click();
    const alert = section.getByRole("alert").last();
    await expect(alert).toContainText("Nothing was proposed. Correct “Chart specification (Vega-Lite)” above");
    await expect(section).not.toContainText("/spec/catalog");
    await expect(spec).toHaveAttribute("aria-invalid", "true");
    await expect(spec).toHaveAccessibleDescription(/may use only this endpoint's data\. Replace "url": "https:\/\/example\.org\/no2\.csv" with "name": "source"/);
    // The shots show the member that was refused, at the top of the box.
    await spec.evaluate((box) => (box.scrollTop = 0));
    await alert.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-refused-banner-${width}.png`) });
    await section.getByRole("button", { name: "Go to the field" }).click();
    await expect(spec).toBeFocused();
    await spec.evaluate((box) => (box.scrollTop = 0));
    await page.screenshot({ path: join(DIR, `toolbar-refused-${width}.png`) });
  });

  test(`a saved chart that blocks a removal is named at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await stub(page, { verbs: ["read", "propose", "approve"], charts: [SHOWN, BROKEN], puts: [], refuse: OVER_31_DAYS });
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByRole("button", { name: "Remove" }).first().click();
    await expect(section.getByRole("button", { name: "Remove “NO2 this quarter”" })).toBeVisible();
    await expect(section.getByRole("button", { name: "Propose 31 days for “NO2 this quarter”" })).toBeVisible();
    await expect(section.getByRole("alert").filter({ hasText: "was not removed" })).toContainText("“NO2 this quarter”");
    await expect(section.getByText("Range: 90 days · Granularity: Daily · Method: Maximum")).toBeVisible();
    await expect(section.getByText("90 days is not an allowed range")).toBeVisible();
    await expect(section).not.toContainText("/spec/catalog");
    await section.getByRole("alert").filter({ hasText: "was not removed" }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-refused-other-${width}.png`) });
  });

  test(`a refusal is said in the person's language at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await stub(page, { verbs: ["read", "propose", "approve"], charts: [SHOWN, BROKEN], puts: [], refuse: OVER_31_DAYS });
    await page.goto("/projects/ovzdusie/endpoints/air?lang=sk");
    const section = page.getByTestId("visualizations");
    await section.getByRole("button", { name: "Odstrániť" }).first().click();
    await expect(section.getByText("Obdobie „90 dní“ nie je povolené")).toBeVisible();
    await expect(section).not.toContainText("/spec/catalog");
    await section.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-refused-sk-${width}.png`) });
    await section.locator("summary").first().click();
    await shootForm(page, `toolbar-form-sk-${width}.png`, width);
  });

  test(`a chart that does not read the history has no toolbar settings at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await stub(page, { verbs: ["read", "propose", "approve"], charts: [], puts: [] });
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await section.getByText("Add a chart").click();
    await page.getByRole("combobox", { name: "Chart", exact: true }).selectOption("bar");
    await expect(section.getByTestId("viz-temporal")).toHaveCount(0);
    await shootForm(page, `toolbar-bar-${width}.png`, width);
  });

  test(`a reader without the propose right sees the charts and no editor at ${width} px (T-7491)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await stub(page, { verbs: ["read"], charts: [SHOWN, CHART, FIXED], puts: [] });
    await page.goto("/projects/ovzdusie/endpoints/air?lang=en");
    const section = page.getByTestId("visualizations");
    await expect(section.getByText("NO2 this week")).toBeVisible();
    await expect(section.getByText("Add a chart")).toHaveCount(0);
    await expect(section.getByText("Range: 24 hours · Granularity: Hourly · Method: Average")).toBeVisible();
    await expect(section.getByText("Range: 7 days · Granularity: Daily · Method: Maximum")).toBeVisible();
    await expect(section.getByText("Range: 24 hours · Granularity: Raw readings")).toBeVisible();
    await section.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(DIR, `toolbar-reader-${width}.png`) });
  });
  });
}
