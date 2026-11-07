import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6089: the Retention tab as the owner walked it on dev Helsinki. Types under one identical rule
// share a row, an onChange heartbeat reads "a row at least every 30 days", the deleted column says
// it counts since the broker started, and an attribute without a unit stamped on receipt says why
// its rows still match. `vite preview` has no Portal API, so the API is answered in the browser;
// with SHOTS_DIR set, the 1440 px and 400 px screenshots are written there.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const BASE = `/api/v1/projects/${PROJECT}/spaces/${SPACE}`;
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const SPACE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: SPACE, namespace: PROJECT, title: { en: "Helsinki city context" } },
  spec: {
    retention: [
      { entityType: "Vehicle", current: "PT5M", history: "PT5M" },
      { entityType: "Alert", current: "PT1H" },
      { entityType: "KeyPerformanceIndicator", history: "P90D", record: "onChange", heartbeat: "P30D" },
      { entityType: "AirQualityObserved", history: "P30D", record: "onChange", heartbeat: "PT1H" },
    ],
  },
  status: { phase: "Live" },
};
const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "helsinki-model", namespace: PROJECT },
  spec: {
    contextSpaceRef: SPACE,
    classes: ["Building", "CulturalEvent", "Museum", "Park", "PointOfInterest", "Road", "Streetlight", "Vehicle", "Alert"],
  },
};
const AUDIT = {
  rows: [
    {
      pipeline: "air-quality",
      entityType: "AirQualityObserved",
      attribute: "dateObserved",
      record: "onChange",
      observedAt: ["received"],
      unitCode: [],
      datasetId: [],
      history: "P30D",
      lifetime: [],
      verdict: "ok",
    },
  ],
  unread: [],
  verdicts: { ok: 1, "loses-history": 0, "unbounded-growth": 0, "duplicates-rows": 0, "stamped-on-receive": 0 },
};

async function open(page: Page, width: number, height: number): Promise<void> {
  await page.setViewportSize({ width, height });
  await page.route("**/{api,cs}/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path === `${BASE}/retention/deleted`) return json({ counts: [{ entityType: "Road", kind: "entity", count: 12 }] });
    if (path === `${BASE}/temporal-audit`) return json(AUDIT);
    if (path.endsWith("/datamodels")) return json(list([MODEL]));
    if (path === BASE) return json(SPACE_MANIFEST);
    return json(list([]));
  });
  await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?tab=retention&lang=en`);
  await expect(page.getByRole("table", { name: "How long each type's data is kept" })).toBeVisible();
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`types under one rule share a row, durations in words (${label})`, async ({ page }) => {
    await open(page, width, height);
    const table = page.getByRole("table", { name: "How long each type's data is kept" });
    await table.scrollIntoViewIfNeeded();
    const dir = process.env.SHOTS_DIR;
    if (dir) await page.screenshot({ path: `${dir}/retention-grouped-${label}.png`, fullPage: true });
    const same = table.getByRole("row").filter({ hasText: "7 types, same rule" });
    await expect(same.getByText("12 values, 0 history records")).toBeVisible();
    await same.getByText("7 types, same rule").click();
    await expect(same.getByText("Building, CulturalEvent, Museum, Park, PointOfInterest, Road, Streetlight")).toBeVisible();
    const kpi = table.getByRole("row").filter({ hasText: "KeyPerformanceIndicator" });
    await expect(kpi.getByText("a row at least every 30 days")).toBeVisible();
    await expect(page.getByText(/Counted by the broker since it last started/)).toBeVisible();
    const audit = page.getByRole("table", { name: "How each pipeline's writes become history rows" });
    await expect(audit.getByRole("row").filter({ hasText: "dateObserved" }).getByText(/fine for a value without a unit/)).toBeVisible();
    await expect(page.getByText("Measurement stamped on receipt: 0")).toBeVisible();
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(sideways).toBeLessThanOrEqual(0);
    expect(await axeViolations(page)).toEqual([]);
    if (dir) await page.screenshot({ path: `${dir}/retention-grouped-open-${label}.png`, fullPage: true });
  });
}
