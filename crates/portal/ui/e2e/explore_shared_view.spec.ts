import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import { axeViolations } from "./axe";

// T-7072: two findings of the grid review on Explore. A condition set in the grid's own filter row
// travels in the shared link, and a relationship followed from a row's detail can be walked back.
// `vite preview` has no API and no gateway, so the browser answers both.

const TYPE = "BikeHireDockingStation";
const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const STATION_ID = "urn:ngsi-ld:Station:hel.fi:mobility:central";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, availableBikeNumber, name, refStation]",
  "slots:",
  "  id: {}",
  "  availableBikeNumber: { range: integer }",
  "  name: { range: string }",
  "  refStation: { range: string }",
  "",
].join("\n");

const dock = (n: number, bikes: number) => ({
  id: `urn:ngsi-ld:${TYPE}:hel.fi:mobility:00${n}`,
  type: TYPE,
  availableBikeNumber: { type: "Property", value: bikes },
  name: { type: "Property", value: `Dock ${n}` },
  refStation: { type: "Relationship", object: STATION_ID },
});
const DOCKS = [dock(1, 0), dock(2, 7)];
const STATION = { id: STATION_ID, type: "Station", name: { type: "Property", value: "Central" } };

async function stub(page: Page): Promise<string[]> {
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      route.fulfill({ status: 200, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.viewer", name: "Demo Viewer", roles: [], groups: ["helsinki"] });
    if (path.endsWith("/access/check")) return json({ decision: false });
    if (path.endsWith("/access/v1/evaluations")) {
      return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as unknown[]).map(() => ({ decision: false })) });
    }
    if (path.endsWith("/access/attributes")) return json(attributeTable(TYPE, ["availableBikeNumber", "name", "refStation"]));
    if (path.endsWith("/access")) {
      return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity"], attributes: "*" }, { resource: { type: "Station" }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] });
    }
    if (path.includes("/temporal/entities/")) return json({});
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      if (url.searchParams.get("type") === "Station") return json([STATION], { "NGSILD-Results-Count": "1" });
      const q = url.searchParams.get("q") ?? "";
      const rows = q.includes("availableBikeNumber==0") ? DOCKS.filter((d) => d.availableBikeNumber.value === 0) : DOCKS;
      return json(rows, { "NGSILD-Results-Count": String(rows.length) });
    }
    if (path.includes("/entities/")) {
      if (path.includes(STATION_ID)) return json(STATION);
      return json(DOCKS.find((d) => path.includes(d.id)) ?? DOCKS[0]);
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" } }, spec: { dataModelRef: "helsinki-mobility" }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } }, spec: { contextSpaceRef: "mobility", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld"] }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-mobility", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reads;
}

const SHOTS = process.env.T7072_SHOTS ?? "test-results/T-7072";
mkdirSync(SHOTS, { recursive: true });
const ADDRESS = `/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&lang=en`;

for (const width of [1440, 400]) {
  test(`the link carries the grid's filter at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page);
    await page.goto(ADDRESS);
    await expect(page.getByRole("button", { name: "Open: Dock 2" }).first()).toBeVisible();
    // A phone folds the filter row under its button.
    if (width < 640) await page.locator(".jc-grid-filter-toggle").click();
    await page.getByLabel("Filter: availableBikeNumber").selectOption("equals");
    await page.getByLabel("Value: availableBikeNumber").fill("0");
    await expect(page.getByRole("button", { name: "Open: Dock 2" })).toHaveCount(0);
    await expect(page).toHaveURL(/q=availableBikeNumber%3D%3D0/);

    // A colleague opening the copied link reads the same rows.
    const link = page.url();
    reads.length = 0;
    await page.goto(link);
    await expect(page.getByRole("button", { name: "Open: Dock 1" }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: "Open: Dock 2" })).toHaveCount(0);
    expect(reads.some((search) => search.includes("availableBikeNumber%3D%3D0"))).toBe(true);
    expect(link).not.toMatch(/token|bearer/i);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/shared-filter-${width}.png`, fullPage: true });
  });

  test(`the link carries the order at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page);
    await page.goto(ADDRESS);
    await expect(page.getByRole("button", { name: "Open: Dock 2" }).first()).toBeVisible();
    await page.getByRole("button", { name: "Sort all rows by availableBikeNumber" }).click();
    await expect(page).toHaveURL(/sort=availableBikeNumber(&|$)/);

    // Reopened, the link asks the source in the same order.
    const link = page.url();
    reads.length = 0;
    await page.goto(link);
    await expect(page.getByRole("button", { name: "Open: Dock 2" }).first()).toBeVisible();
    expect(reads.some((search) => search.includes("orderBy=availableBikeNumber"))).toBe(true);
  });

  test(`follows a relationship and comes back at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page);
    await page.goto(ADDRESS);
    await page.getByRole("button", { name: "Open: Dock 1" }).first().click();
    const dock = page.getByRole("dialog", { name: "Details: Dock 1" });
    await expect(dock).toBeVisible();
    await expect(dock.getByRole("button", { name: "Back to the entity you came from" })).toHaveCount(0);

    await dock.getByRole("button", { name: /^Open: Station/ }).click();
    const station = page.getByRole("dialog", { name: "Details: Central" });
    await expect(station).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`entityId=${encodeURIComponent(STATION_ID)}`));
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/relationship-target-${width}.png`, fullPage: true });

    await station.getByRole("button", { name: "Back to the entity you came from" }).click();
    await expect(page.getByRole("dialog", { name: "Details: Dock 1" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Back to the entity you came from" })).toHaveCount(0);
    await page.screenshot({ path: `${SHOTS}/relationship-back-${width}.png`, fullPage: true });
  });
}
