import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// T-6205: record lists moved onto the shared data grid (Organization brokers and data models, an
// upload source's files, an App's scheduled jobs), filled, empty and failed. `vite preview` has
// no Portal API behind it, so the API is answered in the browser. JC_SHOTS=<dir> saves a desktop
// 1440 and a phone 400 shot of each state.

type State = "filled" | "empty" | "error";
const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const broker = (name: string, extra: Record<string, unknown>) => ({
  name, declared: true, default: false, size: "small", features: [], health: "up", version: "0.14.2", reason: null,
  spaces: [], entities: 0, observedAt: "2026-10-31T08:00:00Z", ...extra,
});
const BROKERS = [
  broker("main", { default: true, spaces: ["ilma/air", "liikenne/stops"], entities: 791_784 }),
  broker("canary", { health: "down", version: null, reason: "the broker did not answer; try again in a minute" }),
  broker("edge", { spaces: ["pysakointi/lots"], entities: 1_204, version: "0.14.1" }),
];
const MODELS = [
  { name: "stations", level: "organization", project: "org", version: "2.1.0", lifecycle: "published", classes: ["Station", "Dock"] },
  { name: "parks", level: "organization", project: "org", version: "0.2.0", lifecycle: "draft", classes: ["Park"] },
  { name: "transit", level: "organization", project: "org", version: "1.0.0", lifecycle: "published", classes: ["Stop", "Line", "Trip"] },
];
const FILES = [
  { id: 7, name: "rozpocet-2026.xlsx", mime: "application/xlsx", bytes: 48_000, passages: 12, status: "fetched" },
  { id: 8, name: "zapisnica-2026-10-12.pdf", mime: "application/pdf", bytes: 920_000, passages: 0, status: "failed" },
  { id: 9, name: "uznesenia.docx", mime: "application/docx", bytes: 20_000, passages: 40, status: "fetched" },
];
const SOURCE = { source: "hel-files", type: "upload", state: "crawled", startUrls: [], ckanInstanceRef: null, schedule: null, visibility: "public", documents: 3, passages: 52, embedded: 52, pending: 0 };
const APP = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: { name: "kpi", namespace: "helsinki" },
  spec: { kind: "wasm", title: "City KPIs", server: { jobs: [
    { name: "hourly", schedule: "0 * * * *", export: "compute" },
    { name: "nightly", schedule: "15 2 * * *", export: "rollup" },
    { name: "weekly", schedule: "0 3 * * 1", export: "archive" },
  ] } },
  status: { phase: "Live", jobs: [
    { name: "hourly", lastRun: "2026-10-31T14:00:00Z", outcome: "succeeded", durationMs: 42, failuresInARow: 0 },
    { name: "nightly", lastRun: "2026-10-31T02:15:00Z", outcome: "failed", message: "403 from the Endpoint: the indicator type is not granted", durationMs: 120, failuresInARow: 3 },
  ] },
};

async function stubApi(page: Page, state: State): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const failed = () => json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "the service did not answer in time; try again in a minute" }, 503);
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = ["read", "propose", "approve", "delete"];
      const grants = [{ role: "admin", binding: "b", rule: { kinds: ["RoleBinding"], verbs } }, { role: "admin", binding: "b", rule: { kinds: ["*", "KnowledgeSource"], verbs } }];
      return json({ project: path.split("/")[4] ?? "org", bootstrap: false, grants });
    }
    if (path === "/api/v1/organization/brokers") return state === "error" ? failed() : json({ items: state === "empty" ? [] : BROKERS });
    if (path === "/api/v1/organization/datamodels") return state === "error" ? failed() : json({ apiVersion: "v1", kind: "OrganizationModels", items: state === "empty" ? [] : MODELS, smartDataModels: [] });
    if (path === "/api/v1/projects/helsinki/knowledge/sources") return json({ items: [SOURCE] });
    if (path === "/api/v1/projects/helsinki/knowledge/sources/hel-files/files") return state === "error" ? failed() : json({ items: state === "empty" ? [] : FILES });
    if (path === "/api/v1/projects/helsinki/apps/kpi") return state === "error" ? failed() : json(APP);
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "desktop"], [400, "phone"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: join(dir, `${name}-${label}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

const LISTS = [
  { name: "org-brokers", path: "/organization/brokers", caption: "Context brokers", first: "main", search: "pysakointi", kept: "edge", empty: /has no broker yet/ },
  { name: "org-models", path: "/organization/models", caption: "The organization's data models", first: "stations", search: "trip", kept: "transit", empty: /shares no data model yet/ },
  { name: "upload-files", path: "/projects/helsinki/knowledge/hel-files", caption: "Files", first: "rozpocet-2026.xlsx", search: "uznes", kept: "uznesenia.docx", empty: /No file yet/ },
  { name: "app-jobs", path: "/projects/helsinki/apps/kpi", caption: "Scheduled jobs of kpi and their last runs", first: "hourly", search: "night", kept: "nightly", empty: /$^/ },
] as const;

for (const each of LISTS) {
  test(`${each.name}: the grid searches and opens a row's details`, async ({ page }) => {
    await stubApi(page, "filled");
    await page.goto(each.path);
    const table = page.getByRole("table", { name: each.caption });
    await expect(table.getByText(each.first, { exact: true })).toBeVisible();
    await shot(page, `${each.name}-filled`);
    const grid = page.locator(`[data-manifest-grid]`).filter({ has: table });
    await grid.getByRole("searchbox").fill(each.search);
    await expect(table.getByText(each.kept, { exact: true })).toBeVisible();
    await expect(table.getByText(each.first, { exact: true })).toHaveCount(0);
    await grid.getByRole("searchbox").fill("");
    // The row's one control is its More actions menu since the grid rework; Details is its first item.
    await table.getByRole("button", { name: `More actions for ${each.first}` }).click();
    await page.getByRole("menuitem", { name: "Details" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await shot(page, `${each.name}-details`);
  });

  if (each.name === "app-jobs") continue; // an App without jobs shows no list, and a failed read is the App page's own
  for (const state of ["empty", "error"] as const) {
    test(`${each.name}: the ${state} state`, async ({ page }) => {
      await stubApi(page, state);
      await page.goto(each.path);
      if (state === "error") await expect(page.getByText(/did not answer in time/).first()).toBeVisible();
      else await expect(page.getByText(each.empty).first()).toBeVisible();
      await page.waitForTimeout(300);
      await shot(page, `${each.name}-${state}`);
    });
  }
}
