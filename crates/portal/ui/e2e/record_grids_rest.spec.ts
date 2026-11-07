import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// T-6206: the rest of the record lists on the shared data grid (all endpoints, workspaces,
// knowledge sources, conversations, CKAN catalogues, roles, health log), filled, empty and
// failed. `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

type State = "filled" | "empty" | "error";
const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, project: string, title: string, audience: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: project, title: { en: title }, labels: { "joinedcontext.com/space": "ilma" } },
  spec: { contextSpaceRef: "ilma", slug: `k7m2qz4tv6xh3n5jb2ryd3wcf${name.length}`, audience, enabledRepresentations: ["ngsi-ld", "csv"] },
  status: { phase: "Live" },
});
const ENDPOINTS = [
  endpoint("air-public", "helsinki", "Air quality", "public"),
  endpoint("noise", "espoo", "Noise sensors", "organization"),
  endpoint("parking", "vantaa", "Parking lots", "project-list"),
];
const workspace = (name: string, title: string, expiresAt: string, previewState: string) => ({
  name, title, owner: "ada@hel.fi", scope: { kind: "project" }, expiresAt, previewState,
});
const WORKSPACES = [
  workspace("ws-air", "Air quality rework", "2026-11-12T00:00:00Z", "running"),
  workspace("ws-noise", "Noise thresholds", "2026-11-03T00:00:00Z", "error"),
  workspace("ws-stops", "Bus stops", "2026-11-20T00:00:00Z", "stopped"),
];
const source = (name: string, type: string, state: string, passages: number, extra: Record<string, unknown> = {}) => ({
  source: name, type, state, startUrls: type === "website" ? ["https://www.hel.fi/"] : [], ckanInstanceRef: null, schedule: null,
  visibility: "public", pages: passages ? 120 : 0, pagesIncluded: passages ? 110 : 0, documents: 4, passages, embedded: passages,
  pending: 0, failed: 0, lastCrawl: passages ? "2026-10-31T03:00:00Z" : null, languages: passages ? { fi: 80, en: 30 } : {},
  job: passages ? { state: "done", attempts: 1, error: null } : null, ...extra,
});
const SOURCES = [
  source("hel-web", "website", "crawled", 900),
  source("hel-news", "website", "crawled", 40),
  source("hel-data", "catalogue", "not-crawled", 0),
];
const conversation = (id: string, question: string, tokensIn: number, timeouts: number) => ({
  id, deployment: "citizen-desk", origin: null, startedAt: "2026-10-31T09:00:00Z", lastAt: "2026-10-31T09:01:00Z",
  firstQuestion: question, turns: 2, routes: ["data"], totalMs: 2400, errors: 0, timeouts, costUsd: 0.004, uncosted: 0,
  thumbsUp: 0, thumbsDown: 0, tokensIn, tokensOut: 230, cachedTokens: 0, firstTokenMs: 800, models: ["model-a"],
});
const CONVERSATIONS = [
  conversation("11111111-1111-4111-8111-111111111111", "What is the PM10 today?", 58_722, 0),
  conversation("22222222-2222-4222-8222-222222222222", "Who is the mayor?", 5_560, 1),
];
const role = (name: string, kinds: string[], verbs: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1", kind: "Role", metadata: { name, namespace: "helsinki" }, spec: { rules: [{ kinds, verbs }] },
});
const check = (name: string, kind: string, status: string) => ({
  project: "helsinki", kind, name, check: "reconcile", status, since: "2026-10-31T08:00:00Z", lastOk: status === "ok" ? "2026-10-31T09:00:00Z" : null,
  detail: { text: status === "ok" ? "" : "the target Endpoint refused the write: 403 no policy grants", code: null, args: {} },
  fix: { text: "", code: null, args: {} }, evidenceUrl: `/projects/helsinki/${kind}s/${name}`, history: [{ at: "2026-10-31T08:00:00Z", status }],
});

async function stubApi(page: Page, state: State): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const failed = () => json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "the service did not answer in time; try again in a minute" }, 503);
    const answer = (items: unknown) => (state === "error" ? failed() : json(items));
    const rows = (items: unknown[]) => (state === "empty" ? [] : items);
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = ["read", "propose", "approve", "delete"];
      const project = path.split("/")[4] ?? "org";
      // Named kinds and the project scope: `can()` matches `*` only for kind `*` (KNOWLEDGE, T-5591, T-6105).
      const kinds = ["RoleBinding", "Role", "AssistantDeployment", "KnowledgeSource", "CkanInstance", "Endpoint", "*"];
      const grants = [{ role: "admin", binding: "b", scope: `project:${project}`, rule: { kinds, verbs } }];
      return json({ project, bootstrap: false, grants });
    }
    if (path === "/api/v1/endpoints") return answer(list(rows(ENDPOINTS)));
    if (path === "/api/v1/projects/helsinki/workspaces") return answer(list(rows(WORKSPACES)));
    if (path === "/api/v1/projects/helsinki/knowledge/sources") return answer({ items: rows(SOURCES) });
    if (path === "/api/v1/projects/helsinki/knowledge/conversations") return answer({ items: rows(CONVERSATIONS), nextCursor: null });
    if (path === "/api/v1/projects/helsinki/ckan/status") {
      const instance = (name: string, url: string, organizationDefault: string) => ({ name, url, organizationDefault, apiTokenRef: `ckan-${name}` });
      return answer({ instances: rows([instance("open-data", "https://data.hel.fi", "helsinki"), instance("archive", "https://archive.hel.fi", "archive")]), publications: [] });
    }
    if (path === "/api/v1/projects/helsinki/roles") return answer(list(rows([role("air-steward", ["Pipeline"], ["propose"]), role("air-analyst", ["DataSource"], ["read"])])));
    if (path === "/api/v1/projects/helsinki/health-checks") {
      return answer({ items: rows([check("air-intake", "pipeline", "error"), check("air-public", "endpoint", "warn"), check("ilma", "space", "ok")]) });
    }
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
  { name: "all-endpoints", path: "/organization/endpoints", caption: "All endpoints", first: "Air quality", search: "noise", kept: "Noise sensors", empty: /No project publishes an endpoint/ },
  { name: "workspaces", path: "/projects/helsinki/workspaces", caption: "Copies of this project", first: "Air quality rework", search: "bus", kept: "Bus stops", empty: /No copies/ },
  { name: "knowledge-sources", path: "/projects/helsinki/knowledge", caption: "Knowledge sources", first: "hel-web", search: "news", kept: "hel-news", empty: /has no knowledge source/ },
  { name: "conversations", path: "/projects/helsinki/knowledge/conversations", caption: "Conversations", first: "What is the PM10 today?", search: "mayor", kept: "Who is the mayor?", empty: /No stored conversations/ },
  { name: "ckan", path: "/projects/helsinki/ckan", caption: "Catalogues", first: "open-data", search: "archive", kept: "archive", empty: /No catalogue/ },
  { name: "roles", path: "/projects/helsinki/settings/roles", caption: "Roles in force in helsinki", first: "air-steward", search: "analyst", kept: "air-analyst", empty: /No role is defined/ },
  { name: "health", path: "/projects/helsinki/health", caption: "Health checks", first: "air-intake", search: "public", kept: "air-public", empty: /Nothing checked yet/ },
] as const;

for (const each of LISTS) {
  test(`${each.name}: the grid searches and opens a row's details`, async ({ page }) => {
    await stubApi(page, "filled");
    await page.goto(each.path);
    const table = page.getByRole("table", { name: each.caption });
    await expect(table.getByText(each.first, { exact: true }).first()).toBeVisible();
    await shot(page, `${each.name}-filled`);
    const grid = page.locator(`[data-manifest-grid]`).filter({ has: table });
    await grid.getByRole("searchbox").fill(each.search);
    await expect(table.getByText(each.kept, { exact: true }).first()).toBeVisible();
    await expect(table.getByText(each.first, { exact: true })).toHaveCount(0);
    await grid.getByRole("searchbox").fill("");
    // The row's one control is its More actions menu since the grid rework; Details is its first item.
    await table.getByRole("button", { name: /^More actions for / }).first().click();
    await page.getByRole("menuitem", { name: "Details" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await shot(page, `${each.name}-details`);
  });

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
