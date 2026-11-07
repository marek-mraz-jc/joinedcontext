import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// A dashboard App edited without code (T-6402): on the App page a person opens Edit the
// dashboard, adds a computed column, a filter, a colour rule, a chart and a tile, previews the
// file before and after and proposes it as the App's Change; a refusal of the server stands
// beside the field it names. `vite preview` has no Portal API, so the API is answered here.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const APP = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "App",
  metadata: { name: "bike-board", namespace: "helsinki", title: { en: "Shared bikes" } },
  status: { phase: "Live", build: { digest: "sha256:ab", commit: "9f1c2ab" } },
  spec: { kind: "static", visibility: "project", lifecycle: "published", dataNeeds: [] },
};

const FILE = {
  title: "Shared bikes",
  grid: { source: { kind: "endpoint", slug: "bikes" }, type: "BikeHireDockingStation", columns: [{ attr: "name", label: "Station" }, { attr: "availableBikeNumber", label: "Bikes" }] },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000d4b1", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "App", name: "bike-board", fields: 1 } },
  author: { name: "Eva Editor", email: "eva@hel.fi" },
  createdAt: "2026-11-01T23:30:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 }, mergeRequest: "https://forge.example/helsinki/config/pulls/12" },
};

const text = (value: unknown) => `${JSON.stringify(value, null, 2)}\n`;

async function stub(page: Page, refuse = false): Promise<void> {
  await page.route(/\/api\//, async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: status < 300 ? "application/json" : "application/problem+json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "e1", username: "eva", name: "Eva Editor", email: "eva@hel.fi", roles: [] });
    if (path.endsWith("/branding")) return json({ organisation: "Helsinki", appsOrigin: "https://dev.example.sk" });
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: [{ rule: { kinds: ["App"], verbs: ["read", "propose"] } }] });
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path.endsWith("/apps/bike-board/dashboard") && request.method() === "GET") {
      return json({
        path: "src/dashboard.json",
        config: FILE,
        entityType: "BikeHireDockingStation",
        attributes: ["name", "availableBikeNumber", "totalSlotNumber", "freeSlotNumber", "status"].map((name) => ({ name, kind: "Property", required: false })),
        edit: { allowed: true },
      });
    }
    if (path.endsWith("/apps/bike-board/dashboard")) {
      const body = request.postDataJSON() as { config: unknown; dryRun: boolean };
      if (refuse) {
        return json({ type: "about:blank", title: "Unprocessable Entity", status: 422, detail: "refused", errors: [{ path: "config/grid/columns/2/formula", message: '"fetch(" is a function, which a formula cannot use: write attribute names, numbers and + - * / only (at character 1)' }] }, 422);
      }
      return body.dryRun ? json({ before: text(FILE), after: text(body.config) }) : json({ before: text(FILE), after: text(body.config), change: CHANGE, pullRequestUrl: "https://forge.example/pulls/4" }, 202);
    }
    if (path.endsWith("/apps/bike-board")) return json(APP);
    if (path.endsWith("/changes/chg-0000d4b1")) return json(CHANGE);
    if (path.endsWith("/build")) return json({ title: "Not Found", status: 404 }, 404);
    if (path.endsWith("/app-checks")) return json({ checks: [] });
    if (path.endsWith("/agent-runs")) return json({ items: [] });
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

async function open(page: Page): Promise<void> {
  await page.goto("/projects/helsinki/apps/bike-board?lang=en");
  await page.getByText("Edit the dashboard").click();
  await expect(page.getByRole("group", { name: "Columns" })).toBeVisible();
}

async function fill(page: Page, formula: string): Promise<void> {
  await page.getByRole("button", { name: "Add computed column" }).click();
  await page.locator("#col-2-name").fill("occupancy");
  await page.locator("#col-2-label").fill("Occupancy %");
  await page.locator("#col-2-formula").fill(formula);
  await page.getByRole("button", { name: "Add filter" }).click();
  await page.locator("#filter-0-attr").selectOption("status");
  await page.locator("#filter-0-value").fill("working");
  await page.locator("#colour-attr").selectOption("occupancy");
  await page.getByRole("button", { name: "Add band below" }).click();
  await page.locator("#band-0-below").fill("10");
  await page.locator("#band-0-label").fill("Almost empty");
  await page.locator("#band-1-label").fill("Bikes available");
  await page.locator("#chart-x").selectOption("name");
  await page.locator("#chart-y").selectOption("availableBikeNumber");
  await page.getByRole("button", { name: "Add tile" }).click();
  await page.locator("#kpi-0-label").fill("Empty stations");
  await page.locator("#kpi-0-where").fill("availableBikeNumber == 0");
}

for (const width of [1440, 400]) {
  test(`a person edits a dashboard App, previews and proposes the Change at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page);
    await open(page);
    await shot(page, `${width}-1-editor`);
    await fill(page, "availableBikeNumber / totalSlotNumber * 100");
    await shot(page, `${width}-2-filled`);
    await page.getByRole("button", { name: "Preview change" }).click();
    await expect(page.getByText("What the Change will hold").first()).toBeVisible();
    await shot(page, `${width}-3-preview`);
    expect(await axeViolations(page)).toEqual([]);
    await page.getByRole("button", { name: "Propose change" }).click();
    await expect(page.getByRole("region", { name: /bike-board/ })).toBeVisible();
    await shot(page, `${width}-4-proposed`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });

  test(`a refused formula is shown beside its field at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 860 });
    await stub(page, true);
    await open(page);
    await fill(page, "fetch(name)");
    await page.getByRole("button", { name: "Preview change" }).click();
    await expect(page.locator("#col-2-formula")).toHaveAttribute("aria-invalid", "true");
    const banner = page.getByText("The Change was not proposed. Fix these fields, then preview again:");
    await expect(page.getByText(/^Columns, row 3, Formula: .*is a function, which a formula cannot use/)).toBeVisible();
    await banner.scrollIntoViewIfNeeded();
    await shot(page, `${width}-5-refused`);
    await page.getByRole("button", { name: "Go to the field" }).click();
    await expect(page.locator("#col-2-formula")).toBeFocused();
    await shot(page, `${width}-6-refused-field`);
  });
}
