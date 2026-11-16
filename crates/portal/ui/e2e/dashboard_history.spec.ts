import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// A builder dashboard's history on its page (T-7150): the versions, a comparison, a restore asked
// twice, and a reader who may not restore. `vite preview` has no Portal API behind it, so the API
// is answered in the browser; the pages are the real build.

const UID = "jcd-0123456789abcdef";
const OLD = "1111111111111111111111111111111111111111";
const MID = "3333333333333333333333333333333333333333";
const NEW = "2222222222222222222222222222222222222222";
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const VIEW = {
  uid: UID, orgId: 2, title: "Bike stations", description: "Free bikes per station", from: "now-6h", to: "now", refresh: null,
  variables: [], panels: [{ id: 1, type: "text", title: "About", gridPos: { x: 0, y: 0, w: 12, h: 4 }, options: { content: "Free bikes per station." } }],
};
const board = (title: string, attribute: string, refresh: string) => ({
  title, refresh, from: "now-6h", to: "now", variables: [],
  panels: [{ id: 1, kind: "stat", title: "Free bikes", endpoint: "bikes-open", type: "BikeHireDockingStation", attribute }],
});

async function stubApi(page: Page, mayEdit: boolean, versions: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const base = `/api/v1/projects/helsinki/grafana/builder/${UID}/versions`;
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: "helsinki", sample: false, title: "Helsinki" }]), organizations: [] });
    if (path === `/api/v1/projects/helsinki/grafana/dashboards/${UID}`) return json(VIEW);
    if (path === "/api/v1/projects/helsinki/grafana/builder") return json({ mayEdit, pending: [], ready: true });
    if (path === base) return json({ items: versions });
    if (path === `${base}/${OLD}/restore` && route.request().method() === "POST") return json({ uid: UID, change: { metadata: { name: "pr-12" } } }, 202);
    if (path === `${base}/${OLD}`) return json({ sha: OLD, board: board("Bike stations", "status", "5m") });
    if (path === `${base}/${MID}`) return json({ sha: MID, board: board("Bike stations", "availableBikeNumber", "5m") });
    if (path === `${base}/${NEW}`) return json({ sha: NEW, board: board("Bike stations, per district", "availableBikeNumber", "1m") });
    return json(list([]));
  });
}

const VERSIONS = [
  { sha: NEW, author: "Eva Nováková", date: "2026-11-16T10:00:00Z", message: "change dashboard Bike stations, per district" },
  { sha: MID, author: "Ada Lovelace", date: "2026-11-15T14:20:00Z", message: "change dashboard Bike stations" },
  { sha: OLD, author: "Ada Lovelace", date: "2026-11-14T09:00:00Z", message: "add dashboard Bike stations" },
];

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.getByRole("region", { name: "History" }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: false });
}

test.describe("a dashboard's history", () => {
  for (const width of [1440, 400]) {
    test(`lists, compares and restores asked twice at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true, VERSIONS);
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}?lang=en`);
      const history = page.getByRole("region", { name: "History" });
      await expect(history.getByRole("listitem")).toHaveCount(3);
      await shot(page, `history-list-${width}`);
      await history.getByRole("button", { name: "Compare with current" }).last().click();
      await expect(history.getByText("Bike stations, per district", { exact: true })).toBeVisible();
      await shot(page, `history-diff-${width}`);
      await history.getByRole("button", { name: "Restore", exact: true }).last().click();
      await expect(history.getByText("Restore this version?")).toBeVisible();
      await shot(page, `history-restore-asked-${width}`);
      await history.getByRole("button", { name: "Yes, propose the restore" }).click();
      await expect(history.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/pr-12");
      await shot(page, `history-restore-proposed-${width}`);
    });

    test(`a reader compares but may not restore at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, false, VERSIONS);
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}?lang=en`);
      const history = page.getByRole("region", { name: "History" });
      await expect(history.getByRole("button", { name: "Restore", exact: true }).first()).toBeDisabled();
      await expect(history.getByText(/needs the right to build dashboards/).last()).toBeVisible();
      await shot(page, `history-reader-${width}`);
    });

    test(`an empty history says what comes next at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, true, []);
      await page.goto(`/projects/helsinki/grafana/dashboards/${UID}?lang=en`);
      await expect(page.getByText("No versions yet")).toBeVisible();
      await shot(page, `history-empty-${width}`);
    });
  }
});
