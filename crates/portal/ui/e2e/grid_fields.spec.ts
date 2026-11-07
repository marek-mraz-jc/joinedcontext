import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The space grid shows values by what they hold (T-4827): numbers with their unit, dates in local
// time, a point as coordinates, a relationship as a chip, a structured value as key: value; the
// columns in the model's order with its labels, and a header menu that sorts, filters, hides,
// renames, resizes and moves. `vite preview` has no Portal API behind it, so the API and the space
// surface are answered in the browser. With SHOTS_DIR set, 1440 px and 400 px screenshots are
// written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki-kpi";
const TYPE = "KeyPerformanceIndicator";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT, ...(title ? { title: { en: title } } : {}) },
  spec,
  status: { phase: "Live" },
});

// Smart Data Models' KeyPerformanceIndicator, cut to what the grid shows.
const LINKML = [
  "id: https://hel.fi/models/kpi",
  "name: helsinki-kpi",
  "classes:",
  "  KeyPerformanceIndicator:",
  "    title: Key performance indicator",
  "    slots: [id, name, kpiValue, calculationPeriod, computedBy, dateNextCalculation, location, category]",
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name, rank: 1 }",
  "  kpiValue: { range: float, title: Value }",
  "  calculationPeriod: { range: string, title: Calculation period }",
  "  computedBy: { range: string, title: Computed by, annotations: { ngsi_ld_kind: Relationship } }",
  "  dateNextCalculation: { range: datetime, title: Next calculation }",
  "  location: { range: string, annotations: { ngsi_ld_kind: GeoProperty } }",
  "  category: { range: string, title: Category }",
  "",
].join("\n");

function kpi(n: number) {
  return {
    id: `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:kpi:${n}`,
    type: TYPE,
    name: { type: "Property", value: `District ${n}` },
    kpiValue: { type: "Property", value: 1234.5 * n, unitCode: "P1" },
    calculationPeriod: { type: "Property", value: { from: "2026-10-01T00:00:00Z", to: "2026-10-31T23:59:59Z" } },
    computedBy: { type: "Relationship", object: `urn:ngsi-ld:Organization:hel.fi:kpi:unit-${n}` },
    dateNextCalculation: { type: "Property", value: "2026-11-01T06:00:00Z" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9384 + n / 100, 60.1699] } },
    "ngsi-ld:status": { type: "Property", value: n % 2 ? "ok" : "late" },
  };
}

async function stubApi(page: Page): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path.endsWith("/datamodels/helsinki-kpi/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    }
    if (path.endsWith("/entities")) {
      // The grid reads the targets of a page's relationships once per class; a target it cannot
      // read stays an id, never a link (EP-59).
      if (url.searchParams.get("type") === "Organization") {
        const orgs = (url.searchParams.get("id") ?? "").split(",").filter(Boolean).map((id) => ({ id, type: "Organization", name: { type: "Property", value: id.split(":").pop() } }));
        return json(orgs, 200, { "NGSILD-Results-Count": String(orgs.length) });
      }
      const rows = Array.from({ length: 8 }, (_, i) => kpi(i + 1));
      // The grid asks the broker to order the whole answer (`orderBy`, CIM 009 4.23): answer as it does.
      const [attr, dir] = (url.searchParams.get("orderBy") ?? "").split(";");
      if (attr) {
        const value = (row: object) => String((row as Record<string, { value?: unknown } | undefined>)[attr]?.value ?? "");
        rows.sort((a, b) => value(a).localeCompare(value(b)) * (dir === "desc" ? -1 : 1));
      }
      return json(rows, 200, { "NGSILD-Results-Count": String(rows.length) });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`)) return json({ entities: 8, broker: "main", observedAt: "2026-10-20T16:00:00Z" });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) {
      return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-kpi" }, "Helsinki KPIs"));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-kpi", { contextSpaceRef: SPACE, version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`the grid shows values by field type and the header menu works (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

    const grid = page.getByRole("grid");
    await expect(grid.getByText("District 1", { exact: true })).toBeVisible();
    const headers = grid.locator("thead tr").first().locator(".jc-grid-sort-btn");
    // The ID and Type columns open the grid now; the model's fields follow.
    await expect(headers.first()).toHaveText("ID");
    await expect(headers.nth(1)).toHaveText("Type");
    await expect(headers.nth(2)).toHaveText("Name");
    await expect(headers.nth(3)).toHaveText(/^Value/);
    await expect(headers.nth(4)).toHaveText("Calculation period");
    // No raw JSON, URN or compact IRI in a cell or a header.
    const text = await grid.innerText();
    expect(text).not.toMatch(/[{}]|urn:ngsi-ld|ngsi-ld:/);
    await expect(grid.getByRole("button", { name: "Open: Organization · unit-1" })).toBeVisible();
    await expect(grid.getByText("60.16990, 24.94840")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `grid-fields-${label}`);

    await grid.getByRole("button", { name: "Column options: Value" }).click();
    const menu = page.getByRole("dialog", { name: "Column options: Value" });
    await expect(menu).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `grid-column-menu-${label}`);
    await menu.getByRole("button", { name: "Sort Z → A" }).click();
    await expect(grid.locator("tbody tr").first()).toContainText("District 8");

    await grid.getByRole("button", { name: "Column options: Status" }).click();
    await page.getByRole("dialog", { name: "Column options: Status" }).getByText("Column settings").click();
    await page.getByLabel("Label in this view").fill("State");
    await page.getByLabel("Label in this view").press("Enter");
    await expect(grid.getByRole("button", { name: "Column options: State" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.reload();
    // "All rows" keeps the person's own label in this browser.
    await expect(page.getByRole("grid").getByRole("button", { name: "Column options: State" })).toBeVisible();
  });
}
