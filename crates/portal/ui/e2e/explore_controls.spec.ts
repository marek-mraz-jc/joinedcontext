import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5622: the explorer's controls in plain words. Types the Endpoint grants come first, the rest
// in a group of their own; attributes read by the model's label; an App's Endpoint sits under
// "Applications"; NGSI-LD waits under "Technical details"; a type the space does not have says
// so. `vite preview` has no API, so the browser answers it.

const TYPE = "AirQualityObserved";
const OTHER = "WeatherObserved";
const OWN = { name: "helsinki-air-quality", slug: "jtjnhkaau64zmzz5csmcwem52icvufy2", title: "Helsinki air quality" };
const APP = { name: "app-air-quality", slug: "3jlwl5dg2oidblwqhc6qdi3zxl", title: "Helsinki air quality" };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://hel.fi/models/air",
  "name: helsinki-air",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name, pm10, dateObserved, stewardNote]",
  `  ${OTHER}:`,
  "    slots: [id, name]",
  "slots:",
  "  id: {}",
  "  name: { range: string, annotations: { label: { en: Name } } }",
  "  pm10: { range: float, annotations: { label: { en: PM10 particles } } }",
  "  dateObserved: { range: datetime, annotations: { label: { en: Date observed } } }",
  "  stewardNote: { range: string }",
  "",
].join("\n");

const endpoint = (e: typeof OWN, spec: Record<string, unknown>, annotations?: Record<string, string>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: e.name, namespace: "helsinki", title: { en: e.title }, ...(annotations ? { annotations } : {}) },
  spec: { contextSpaceRef: "helsinki", slug: e.slug, enabledRepresentations: ["ngsi-ld"], ...spec },
  status: { phase: "Live" },
});

async function stub(page: Page): Promise<string[]> {
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      route.fulfill({ status: 200, contentType: "application/json", headers, body: JSON.stringify(body) });
    const path = url.pathname;
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.viewer", name: "Demo Viewer", roles: [], groups: ["helsinki"] });
    if (path.endsWith("/access/check")) return json({ decision: /"(query|retrieve)/.test(route.request().postData() ?? "") });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as { action: { name: string } }[]).map((item) => ({ decision: /^(query|retrieve)/.test(item.action.name) })) });
    if (path.endsWith("/access")) return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }] });
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      const rows = [1, 2, 3].map((n) => ({ id: `urn:ngsi-ld:${TYPE}:hel.fi:helsinki:fmi-${n}`, type: TYPE, name: { type: "Property", value: `Station ${n}` }, pm10: { type: "Property", value: 10 + n }, dateObserved: { type: "Property", value: { "@type": "DateTime", "@value": "2026-10-01T10:00:00Z" } } }));
      return json(rows, { "NGSILD-Results-Count": "3" });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki", title: { en: "Helsinki city context" } }, spec: { dataModelRef: "helsinki-air" }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/endpoints")) {
      const own = endpoint(OWN, { audience: "public" });
      const app = endpoint(APP, { audience: "project-list", callerRole: true }, { "joinedcontext.com/generated-by": "portal/app-reconciler" });
      return json(list([app, own]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-air", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE, OTHER], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reads;
}

const SHOTS = process.env.T5622_SHOTS ?? "test-results/explore-controls";
mkdirSync(SHOTS, { recursive: true });

for (const width of [1440, 400]) {
  test(`the controls speak plain words at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page);
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=${TYPE}&lang=en`);
    await expect(page.locator("tbody tr").first()).toBeVisible();
    // The type, area and filters fold under one line once a type is chosen (T-6107).
    await page.locator("summary", { hasText: "area and filters" }).click();
    await expect(page.locator("#explore-endpoint optgroup")).toHaveAttribute("label", "Applications");
    await expect(page.locator("#explore-type optgroup")).toHaveAttribute("label", "Not readable through this endpoint");
    await expect(page.locator("#explore-type optgroup option")).toHaveText([OTHER]);
    // The model's label reads alone in the Columns menu; the name the data carries is its tooltip (T-6092).
    await page.locator("summary", { hasText: "Columns" }).click();
    const dateColumn = page.getByRole("checkbox", { name: "Date observed" });
    await expect(dateColumn).toBeVisible();
    await expect(dateColumn).toHaveAttribute("title", "dateObserved");
    await page.keyboard.press("Escape");
    await expect(page.getByText("The area code the entities are filed under, e.g. /district/1; leave empty for every area.")).toBeVisible();
    // The query language is out of sight until asked for.
    await expect(page.getByLabel("Query as text (q)")).toBeHidden();
    await page.getByRole("button", { name: "Add filter" }).click();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/controls-${width}.png`, fullPage: true });
    await page.getByTestId("explore-technical").locator("summary").click();
    await expect(page.getByLabel("Query as text (q)")).toBeVisible();
    await page.screenshot({ path: `${SHOTS}/technical-${width}.png`, fullPage: true });
  });

  test(`an unknown type says it does not exist at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page);
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=NoSuchType&lang=en`);
    await expect(page.getByText("There is no type NoSuchType in this space")).toBeVisible();
    await expect(page.locator("tbody tr")).toHaveCount(0);
    expect(reads).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/unknown-type-${width}.png`, fullPage: true });
    // One click on a type the space has opens it.
    await page.getByRole("button", { name: TYPE }).click();
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`type=${TYPE}`));
  });

  test(`a type the Endpoint does not grant stands apart at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page);
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=${OTHER}&lang=en`);
    // The type, area and filters fold under one line once a type is chosen (T-6107).
    await page.locator("summary", { hasText: "area and filters" }).click();
    await page.getByRole("tabpanel", { name: "Entities" }).locator("summary", { hasText: "Your access" }).click();
    await expect(page.getByText(`No grant names ${OTHER} for you through this endpoint.`)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/refused-type-${width}.png`, fullPage: true });
  });
}
