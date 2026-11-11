import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6751: the operational lists (activity, pipeline health, calls per Endpoint) on the shared data
// grid, walked in the built bundle with the API answered from the browser. Each state is shot at
// 1440 and 400 px into test-results/T-6751 for the UI judge.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-admin"] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] };
const at = (hour: number, minute = 0) => `2026-11-11T${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}:00Z`;

const event = (time: string, kind: string, source: string, severity: string, summary: string, object?: string) => ({
  time,
  project: "helsinki",
  kind,
  source,
  severity,
  summary,
  details: object ? { object } : {},
});
const ACTIVITY = [
  event(at(9, 42), "pipeline.error", "reconciler", "error", "Pipeline hsl-vehicles failed: the source answered 503 three times.", "pipelines/hsl-vehicles"),
  event(at(9, 30), "access.denied", "gateway", "warning", "An anonymous caller was refused a write to public-air.", "endpoints/public-air"),
  event(at(9, 12), "change.merged", "portal", "info", "Change chg-5e6f7a8b went live: bus-stops gained 2 attributes.", "endpoints/bus-stops"),
  event(at(8, 55), "pipeline.run", "reconciler", "info", "Pipeline air-quality wrote 412 entities.", "pipelines/air-quality"),
];

const pipeline = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: "helsinki" },
  spec: { class: "scheduled" },
  status: { phase: "Live" },
});
const HEALTH = [
  { pipeline: "air-quality", state: "ok", lastSuccessAt: at(8, 55), lastRunAt: at(8, 55), lastSent: 412, medianSent: 405, changePercent: 2, failedRuns: 0, alert: false },
  { pipeline: "hsl-vehicles", state: "failing", lastSuccessAt: at(6), lastRunAt: at(9, 42), lastSent: 0, medianSent: 1830, changePercent: -100, failedRuns: 3, alert: true, lastError: "GET https://realtime.hsl.fi/hfp: 503 Service Unavailable", lastErrorAt: at(9, 42) },
  { pipeline: "bike-stations", state: "empty", lastSuccessAt: at(9), lastRunAt: at(9), lastSent: 0, medianSent: 0, failedRuns: 0, alert: false },
];
const USAGE = {
  from: "2026-10-13",
  to: "2026-11-11",
  endpoints: [
    { name: "public-air", slug: "public-air", total: 18240, totals: { "ngsi-ld": 16000, odata: 2240 }, daily: [500, 620, 580, 710, 690, 640, 720] },
    { name: "bus-stops", slug: "bus-stops", total: 3120, totals: { "ngsi-ld": 3120 }, daily: [90, 110, 100, 120, 105, 98, 130] },
    { name: "parking", slug: "parking", total: 0, totals: {}, daily: [0, 0, 0, 0, 0, 0, 0] },
  ],
};

type Mode = "filled" | "empty" | "error";

async function stub(page: Page, mode: Mode): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const failed = () =>
      route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503, detail: "The Portal store did not answer. Try again in a minute." }) });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json(PROJECTS);
    if (url.pathname.endsWith("/activity/stream")) return route.fulfill({ status: 204, body: "" });
    if (url.pathname.endsWith("/helsinki/activity")) return mode === "error" ? failed() : json({ items: mode === "empty" ? [] : ACTIVITY });
    if (url.pathname.endsWith("/helsinki/pipelines")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: HEALTH.map((h) => pipeline(h.pipeline)) });
    if (url.pathname.endsWith("/helsinki/pipeline-health")) return mode === "error" ? failed() : json({ items: mode === "empty" ? [] : HEALTH });
    if (url.pathname.endsWith("/helsinki/health-checks")) return json({ items: [] });
    if (url.pathname.endsWith("/helsinki/usage/endpoints"))
      return mode === "error" ? failed() : json(mode === "empty" ? { ...USAGE, endpoints: [] } : USAGE);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const PAGES = [
  { slug: "activity", path: "/projects/helsinki/activity", caption: "Activity", menu: "When", tab: undefined as string | undefined },
  { slug: "pipeline-health", path: "/projects/helsinki/pipelines", caption: "Health of each source", menu: "Newest run sent", tab: "Source health" },
  { slug: "usage", path: "/projects/helsinki/health", caption: "^Usage$", menu: "Calls", tab: undefined },
];

for (const width of [1440, 400]) {
  for (const target of PAGES) {
    test(`${target.slug} on the shared grid at ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      const shot = (state: string) => page.screenshot({ path: `test-results/T-6751/${target.slug}-${state}-${width}.png`, fullPage: true });
      const open = async () => {
        await page.goto(`${target.path}?lang=en`);
        if (target.tab) await page.getByRole("tab", { name: target.tab }).click();
      };

      await stub(page, "filled");
      await open();
      const table = page.getByRole("table", { name: new RegExp(target.caption) });
      await expect(table.locator("tbody tr").first()).toBeVisible();
      await expect(page.locator(".jc-data-grid").first()).toBeVisible();
      await shot("filled");
      expect(await axeViolations(page)).toEqual([]);

      if (width === 1440) {
        await page.getByRole("button", { name: `Column options: ${target.menu}` }).first().click();
        await shot("menu");
        await page.keyboard.press("Escape");
        await page.getByRole("button", { name: "Arrange columns" }).first().click();
        await shot("arrange");
        await page.keyboard.press("Escape");
      }
      await page.getByRole("searchbox", { name: "Search this list" }).first().fill("no-row-says-this");
      await shot("nomatch");

      {
        await page.unrouteAll({ behavior: "ignoreErrors" });
        await stub(page, "empty");
        await open();
        await page.waitForLoadState("networkidle");
        await shot("empty");
        await page.unrouteAll({ behavior: "ignoreErrors" });
        await stub(page, "error");
        await open();
        await expect(page.getByText(/did not answer|could not be read/).first()).toBeVisible();
        await shot("error");
      }
    });
  }
}
