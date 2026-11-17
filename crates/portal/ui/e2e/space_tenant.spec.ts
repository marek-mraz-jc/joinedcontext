import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// T-7216: the Space page shows the broker Tenant the space's data is filed under, read-only, with
// a hint, and a Tenant not known yet as its reason. `vite preview` has no Portal API behind it, so
// the API is answered in the browser; the page is the real build.

test.beforeEach(({ page }) => manifestReadsAsRest(page));

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "mobility", namespace: "helsinki", title: { en: "Helsinki mobility" } },
  spec: {},
  status: { phase: "Live" },
};

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "mobility", namespace: "helsinki", title: "Mobility" },
    spec: { contextSpaceRef: "mobility", version: "1.0.0", classes: ["TrafficFlowObserved"], linkml: "id: https://hel.fi/m\nname: mobility\n" },
  },
]);

// `{OrgId}__{ProjectId}__{space}`: two 26-character ids and the space, as a project-key Tenant reads.
const TENANT = ["01JC7Z0K3M5Q8R2T4V6W8X0Y1Z", "01JC7Z0K4N6P9S3U5W7Y9Z1A2B", "mobility"].join("__");
const REASON = "project `helsinki` has no id in the Portal's project registry yet";

async function stubApi(page: Page, tenant: { tenant: string | null; reason: string | null }): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces/mobility/tenant")) return json(tenant);
    if (path.endsWith("/spaces/mobility/usage")) return json({ entities: 1204, broker: "main", observedAt: "2026-11-17T10:00:00Z" });
    if (path.endsWith("/spaces/mobility")) return json(SPACE);
    if (path.endsWith("/spaces")) return json(list([SPACE]));
    if (path.endsWith("/datamodels")) return json(MODELS);
    return json(list([]));
  });
}

async function shoot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.getByText("Broker tenant", { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `${name}-${width}.png`) });
  }
}

test.describe("the space's broker tenant", () => {
  test("shows the tenant read-only with its hint", async ({ page }) => {
    await stubApi(page, { tenant: TENANT, reason: null });
    await page.goto("/projects/helsinki/spaces/mobility?tab=model&lang=en");
    await expect(page.getByText("Broker tenant", { exact: true })).toBeVisible();
    await expect(page.getByTestId("space-tenant")).toHaveText(TENANT);
    await expect(page.getByRole("textbox", { name: "Broker tenant" })).toHaveCount(0);
    await expect(page.getByText("The name the context broker files this space's data under")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shoot(page, "tenant-filled");
  });

  test("says a tenant not known yet with its reason, never an error page", async ({ page }) => {
    await stubApi(page, { tenant: null, reason: REASON });
    await page.goto("/projects/helsinki/spaces/mobility?tab=model&lang=en");
    await expect(page.getByRole("status").filter({ hasText: `Not known yet: ${REASON}` })).toBeVisible();
    await expect(page.getByTestId("space-tenant")).toHaveCount(0);
    await expect(page.getByRole("heading", { level: 1, name: "Helsinki mobility" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await shoot(page, "tenant-unknown");
  });
});
