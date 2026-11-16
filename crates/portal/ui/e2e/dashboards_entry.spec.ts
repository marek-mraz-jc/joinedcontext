import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// One "Dashboards" entry (T-7160): the builder's folder tree is the list, a dashboard page offers
// "Add visualization", which opens the builder on that dashboard with one new panel to fill, and the
// Dashboard kind's pages sit apart as "Report pages". `vite preview` has no Portal API behind it, so
// the API is answered in the browser; the pages are the real build.

const UID = "jcd-0123456789abcdef";
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const DASHBOARDS = [
  { uid: UID, title: "Bike stations", folder: "Mobility", tags: ["bikes"] },
  { uid: "jcd-1111111111111111", title: "Parking", folder: "Mobility", tags: [] },
  { uid: "jcd-2222222222222222", title: "Air quality", folder: "Environment", tags: ["air"] },
];
const VIEW = {
  uid: UID, orgId: 2, title: "Bike stations", description: "Free bikes per station", from: "now-6h", to: "now", refresh: null,
  variables: [], panels: [{ id: 1, type: "text", title: "About", gridPos: { x: 0, y: 0, w: 12, h: 4 }, options: { content: "Free bikes per station." } }],
};
const START = { ready: true, pending: [], refreshes: ["", "1m", "5m"], units: ["", "none"], colors: ["green", "red"] };
const SAVED = {
  uid: UID, base: "blob-1",
  board: {
    title: "Bike stations", from: "now-6h", to: "now", refresh: "", variables: [],
    panels: [{ id: 1, kind: "stat", title: "Free bikes", grid: { x: 0, y: 0, w: 12, h: 8 }, endpoint: "bikes", type: "BikeHireDockingStation", attribute: "availableBikeNumber", aggregation: "none", interval: "auto" }],
  },
};
const OPTIONS = {
  endpoints: [{ name: "bikes", title: "Bike stations", slug: "bikeslug", public: true, usable: true }],
  types: ["BikeHireDockingStation"],
  attributes: [{ name: "availableBikeNumber", kinds: ["timeseries", "stat", "table"] }],
};

async function stubApi(page: Page, mayEdit: boolean, dashboards: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const base = "/api/v1/projects/helsinki/grafana";
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: "helsinki", sample: false, title: "Helsinki" }]), organizations: [] });
    if (path === base) return json({ ready: true, reason: null, mayCreate: mayEdit, items: [] });
    if (path === `${base}/dashboards`) return json({ ready: true, items: dashboards });
    if (path === `${base}/dashboards/${UID}`) return json(VIEW);
    if (path === `${base}/builder`) return json({ ...START, mayEdit });
    if (path === `${base}/builder/${UID}`) return json(SAVED);
    if (path === `${base}/builder/${UID}/versions`) return json({ items: [] });
    if (path === `${base}/options`) return json(OPTIONS);
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: false, animations: "disabled" });
}

test.describe("one Dashboards entry", () => {
  for (const width of [1440, 400]) {
    test(`lists the dashboards by folder and adds a visualization to one at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true, DASHBOARDS);
      await page.goto("/projects/helsinki/grafana?lang=en");
      await expect(page.getByRole("heading", { level: 1, name: "Dashboards" })).toBeVisible();
      await expect(page.getByRole("button", { name: /Mobility/ })).toBeVisible();
      await expect(page.getByRole("button", { name: /Environment/ })).toBeVisible();
      await expect(page.getByText(/Grafana/)).toHaveCount(0);
      if (width === 400) await page.getByRole("button", { name: "Menu" }).first().click();
      const nav = page.getByRole("navigation", { name: "Main navigation" });
      await expect(nav.getByRole("link", { name: "Dashboards", exact: true })).toHaveAttribute("aria-current", "page");
      await expect(nav.getByRole("link", { name: /Grafana/ })).toHaveCount(0);
      await shot(page, `entry-nav-${width}`);
      if (width === 400) await page.keyboard.press("Escape");
      await page.getByRole("button", { name: /Mobility/ }).click();
      await shot(page, `entry-tree-${width}`);
      await page.getByRole("link", { name: "Bike stations" }).click();
      await page.getByRole("link", { name: "Add visualization" }).click();
      await expect(page.getByRole("heading", { name: "Panel: Panel 2" })).toBeVisible();
      await page.getByRole("heading", { name: "Panel: Panel 2" }).scrollIntoViewIfNeeded();
      await shot(page, `entry-add-visualization-${width}`);
    });

    test(`a reader opens a dashboard without Add visualization at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, false, DASHBOARDS);
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}?lang=en`);
      await expect(page.getByRole("heading", { level: 1, name: "Bike stations" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Add visualization" })).toHaveCount(0);
      await shot(page, `entry-reader-${width}`);
    });

    test(`an empty list offers a new dashboard at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true, []);
      await page.goto("/projects/helsinki/grafana?lang=en");
      await expect(page.getByText("No dashboards yet")).toBeVisible();
      await expect(page.getByRole("link", { name: "New dashboard" })).toBeVisible();
      await shot(page, `entry-empty-${width}`);
    });
  }
});
