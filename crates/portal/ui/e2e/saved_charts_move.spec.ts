import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// The saved charts move into one Saved charts dashboard (T-7161): a check first, then the step it
// names (propose; after the merge, remove what the dashboard holds), and a refusal in words.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.

const UID = "jcd-0123456789abcdef";
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const saved = (id: string, title: string, kind: string) => ({
  id, title, kind, endpoint: "helsinki-air-quality", endpointTitle: "Helsinki air quality", type: "AirQualityObserved",
  attribute: kind === "map" ? "location" : "pm25", createdAt: "2026-11-02T22:00:00Z",
  openUrl: `https://dev.joinedcontext.com/dashboards/d/pv-${id}?orgId=2`,
});
const ITEMS = [saved("ab12", "PM2.5 by station", "timeseries"), saved("cd34", "PM2.5 now", "stat"), saved("ef56", "Stations", "map")];
const KEPT = [{ id: "ef56", title: "Stations", reason: "a map chart: the dashboard builder has no map panel yet" }];

type Stage = "propose" | "remove" | "refused";

async function stubApi(page: Page, stage: Stage): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const base = "/api/v1/projects/helsinki/grafana";
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: "helsinki", sample: false, title: "Helsinki" }]), organizations: [] });
    if (path === base) return json({ ready: true, reason: null, mayCreate: true, items: ITEMS });
    if (path === `${base}/dashboards`) return json({ ready: true, items: [] });
    if (path === `${base}/builder`) return json({ ready: true, mayEdit: true, pending: [], refreshes: [], units: [], colors: [] });
    if (path === `${base}/options`) return json({ endpoints: [], types: [], attributes: [] });
    if (path === `${base}/builder/saved-charts` && request.method() === "POST") {
      const dryRun = (request.postDataJSON() as { dryRun: boolean }).dryRun;
      if (stage === "refused") return json({ status: 409, title: "Conflict", detail: "a change of this dashboard is open already: pr-12; approve or reject it first" }, 409);
      if (stage === "propose") {
        return json({ uid: UID, stage, dryRun, before: 3, panels: 2, moved: ["ab12", "cd34"], kept: KEPT, ...(dryRun ? {} : { change: { metadata: { name: "pr-12" } } }) }, dryRun ? 200 : 202);
      }
      return json({ uid: UID, stage, dryRun, before: 3, panels: 2, moved: ["ab12", "cd34"], kept: KEPT });
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.getByRole("region", { name: "Move into a dashboard" }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: false, animations: "disabled" });
}

test.describe("moving the saved charts", () => {
  for (const width of [1440, 400]) {
    test(`checks, then proposes the Saved charts dashboard at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "propose");
      await page.goto("/projects/helsinki/grafana?lang=en");
      const move = page.getByRole("region", { name: "Move into a dashboard" });
      await move.getByRole("button", { name: "Check the move" }).click();
      await expect(move.getByText(/3 saved charts: the Saved charts dashboard gets 2 panels/)).toBeVisible();
      await expect(move.getByText(/Stations: a map chart/)).toBeVisible();
      await shot(page, `move-checked-${width}`);
      await move.getByRole("button", { name: "Propose the Saved charts dashboard" }).click();
      await expect(move.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/pr-12");
      await shot(page, `move-proposed-${width}`);
    });

    test(`after the merge removes what the dashboard holds at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "remove");
      await page.goto("/projects/helsinki/grafana?lang=en");
      const move = page.getByRole("region", { name: "Move into a dashboard" });
      await move.getByRole("button", { name: "Check the move" }).click();
      await expect(move.getByText(/holds 2 of these charts/)).toBeVisible();
      await shot(page, `move-remove-checked-${width}`);
      await move.getByRole("button", { name: "Remove 2 moved charts" }).click();
      await expect(move.getByText("Removed 2 moved charts")).toBeVisible();
      await shot(page, `move-removed-${width}`);
    });

    test(`says why the move is refused at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "refused");
      await page.goto("/projects/helsinki/grafana?lang=en");
      const move = page.getByRole("region", { name: "Move into a dashboard" });
      await move.getByRole("button", { name: "Check the move" }).click();
      await expect(move.getByText(/a change of this dashboard is open already/)).toBeVisible();
      await shot(page, `move-refused-${width}`);
    });
  }
});
