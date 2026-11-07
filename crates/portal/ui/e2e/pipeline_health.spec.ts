import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// The pipelines page's Source health tab (T-5048) as a steward meets it: every source with its
// state, worst first, and a red source's last error; then the runs of that source. `vite preview`
// has no Portal API behind it, so the API is answered in the browser; the page is the real build.

const IDENTITY = { subject: "s1", username: "demo.steward", name: "Demo Steward", email: "steward@bbsk.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const pipeline = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: "bbsk" },
  spec: { class: "scheduled" },
  status: { phase: "Live", conditions: [{ type: "Scheduled", status: "True" }] },
});

const HEALTH = [
  { pipeline: "podujatia-banskabystrica", state: "ok", lastSuccessAt: "2026-10-21T06:00:10Z", lastRunAt: "2026-10-21T06:00:10Z", lastSent: 64, medianSent: 61, changePercent: 5, failedRuns: 0, alert: false,
    links: { checkedAt: "2026-10-19T02:00:00Z", checked: 118, answered: 117, skipped: 4, deadCount: 0, dead: [] } },
  { pipeline: "podujatia-zvolen", state: "failing", lastSuccessAt: "2026-10-18T06:00:00Z", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 0, medianSent: 10, changePercent: -100, failedRuns: 3, lastError: "GET https://www.zvolen.sk/rss.html answered 404 Not Found", lastErrorAt: "2026-10-21T06:00:01Z", alert: true,
    links: { checkedAt: "2026-10-19T02:00:00Z", checked: 20, answered: 14, skipped: 1, deadCount: 6, dead: [
      { url: "https://www.zvolen.sk/podujatia/jesenny-jarmok-2026", status: 404 },
      { url: "https://www.zvolen.sk/podujatia/koncert-v-parku", status: 404 },
      { url: "https://www.zvolen.sk/podujatia/divadlo-pre-deti", status: 410 },
      { url: "https://www.zvolen.sk/podujatia/vystava-fotografii", status: 404 },
      { url: "https://www.zvolen.sk/podujatia/beh-zvolenom", error: "no answer in 10 s" },
    ] } },
  { pipeline: "podujatia-lucenec", state: "drift", lastSuccessAt: "2026-10-21T06:00:00Z", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 2, medianSent: 12, changePercent: -83, failedRuns: 0, alert: true },
  { pipeline: "podujatia-jelsava", state: "empty", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 0, failedRuns: 0, alert: false },
  { pipeline: "podujatia-detva", state: "failing", lastSuccessAt: "2026-10-20T06:00:00Z", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 0, medianSent: 4, changePercent: -100, failedRuns: 1, lastError: "step 2: expected array, got object from method filter", lastErrorAt: "2026-10-21T06:00:02Z", alert: false },
  { pipeline: "podujatia-novy", state: "never", failedRuns: 0, alert: false },
  // T-5291: still sending, but a step threw on 13 of the hour's 120 records.
  { pipeline: "slovensko-podujatia-casta", state: "failing", lastSuccessAt: "2026-10-21T06:00:00Z", lastRunAt: "2026-10-21T06:00:00Z", lastSent: 107, medianSent: 110, changePercent: -3, failedRuns: 0, lastError: "step 3 threw: date 31.09.2026 is not a calendar day", lastErrorAt: "2026-10-21T06:00:03Z", errors: { errors: 13, records: 120, percent: 10.8 }, alert: true },
];

async function stubApi(page: Page, health: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "bbsk", grants: [{ rule: { kinds: ["Pipeline"], verbs: ["read", "propose", "approve"] } }] });
    }
    if (path.endsWith("/pipeline-health")) return json({ items: health });
    if (path.endsWith("/pipelines")) return json(list(HEALTH.map((h) => pipeline(h.pipeline))));
    if (path.endsWith("/podujatia-zvolen/runs")) {
      return json({ items: [
        { run: "2026-10-21T06:00:00Z", firstAt: "2026-10-21T06:00:00Z", lastAt: "2026-10-21T06:00:01Z", sent: 0, rejected: 0, failed: 1 },
        { run: "2026-10-20T06:00:00Z", firstAt: "2026-10-20T06:00:00Z", lastAt: "2026-10-20T06:00:01Z", sent: 0, rejected: 0, failed: 1 },
      ] });
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("the source health tab (T-5048)", () => {
  for (const width of [1440, 400]) {
    test(`a steward finds the broken sources first and opens one's runs at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, HEALTH);
      await page.goto("/projects/bbsk/pipelines?lang=en");
      await page.getByRole("tab", { name: "Source health" }).click();
      await expect(page).toHaveURL(/view=health/);
      const table = page.getByRole("table", { name: "Health of each source" });
      const rows = table.getByRole("row");
      await expect(rows).toHaveCount(HEALTH.length + 1);
      await expect(rows.nth(1)).toContainText("podujatia-zvolen");
      await expect(rows.nth(1)).toContainText("3 failed runs in a row");
      await expect(rows.nth(1)).toContainText("https://www.zvolen.sk/rss.html answered 404");
      await expect(page.getByText("4 of 7 sources need a look; 3 are alerted.")).toBeVisible();
      // The weekly link check (T-5133): the share that answers, the first dead links and the fix.
      await expect(rows.nth(1)).toContainText("14 of 20 answer (70 %)");
      await expect(rows.nth(1)).toContainText("Dead links: 6");
      await expect(rows.nth(1)).toContainText("and 1 more");
      await expect(rows.nth(1)).toContainText("update the address in its data source");
      await expect(table.getByText("117 of 118 answer (99 %)")).toBeVisible();
      await expect(rows.nth(2)).toContainText("slovensko-podujatia-casta");
      await expect(rows.nth(2)).toContainText("10.8 % failed in the last hour (13 of 120)");
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `health-filled-${width}`);
      await rows.nth(1).getByRole("button", { name: "Runs and log of podujatia-zvolen" }).click();
      await expect(page.getByRole("dialog", { name: "Runs of podujatia-zvolen" })).toBeVisible();
      await shot(page, `health-runs-${width}`);
    });

    test(`a project without pipelines says what to do at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
      await stubApi(page, []);
      await page.goto("/projects/bbsk/pipelines?view=health&lang=en");
      await expect(page.getByText("No pipelines to check yet")).toBeVisible();
      await shot(page, `health-empty-${width}`);
    });
  }
});
