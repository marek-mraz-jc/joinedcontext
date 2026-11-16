import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// EP-187 (T-6972): an open-data Endpoint only reads. The reconciler marks it `OpenData` (the
// page's badge) and, when it binds a write Policy, `Degraded` (the warning with the server's
// sentence); the edit form says what open data means and shows the server's refusal of a change
// that would bind a write. `vite preview` has no Portal behind it: the API is answered here.

const PROJECT = "helsinki";
const NAME = "air-open";
const SLUG = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const WHY =
  "Endpoint air-open: an open-data Endpoint is read-only (EP-187): Policy sensors-write grants updateAttrs; serve the write through an internal Endpoint of the space, or drop updateAttrs from the Policy";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const condition = (type: string, reason: string, message: string) => ({
  type,
  status: "True",
  reason,
  message,
  lastTransitionTime: "2026-11-15T08:00:00Z",
});
const endpoint = (conditions: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT, title: "Air quality, open data" },
  spec: { contextSpaceRef: "air", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld", "mcp"] },
  status: { phase: "Live", conditions },
});
const policy = (name: string, assignee: unknown, operations: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: PROJECT },
  spec: { contextSpaceRef: { kind: "ContextSpace", name: "air" }, assigner: "did:web:hel.fi", assignee, operations },
});
const PUBLIC_READ = policy("public-read", { kind: "role", id: "public" }, ["queryEntity", "retrieveEntity"]);
const SENSORS_WRITE = policy("sensors-write", { kind: "serviceAccount", id: "sensors" }, ["updateAttrs"]);
const MARKED = [
  condition("OpenData", "ReadOnly", "open data: every write through this Endpoint is refused (EP-187)"),
  condition("Degraded", "OpenDataWritePolicy", WHY),
];

async function stub(page: Page, conditions: unknown[]): Promise<void> {
  const manifest = endpoint(conditions);
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const method = route.request().method();
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.startsWith("/api/endpoint/")) return json({}, 404);
    if (path.endsWith("/auth/me"))
      return json({ subject: "s", username: "jana", name: "Jana Example", roles: ["portal-editor"] });
    if (path === "/api/v1/projects") return json({ items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith(`/endpoints/${NAME}/admission`)) return json({ admitted: true });
    if (path.includes(`/drafts/Endpoint/${NAME}`))
      return method === "PUT"
        ? json({ manifest: route.request().postDataJSON()?.manifest, version: 1, updatedAt: "2026-11-15T08:00:00Z" })
        : json({ title: "Not Found", status: 404 }, 404);
    if (path.endsWith(`/${PROJECT}/policies`)) return json(list([PUBLIC_READ, SENSORS_WRITE]));
    if ((path.includes(`/endpoints/${NAME}`) || path.endsWith(`/${PROJECT}/endpoints`)) && method !== "GET")
      return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: WHY, errors: [`spec.contextSpaceRef: ${WHY}`] }, 422);
    if (path.endsWith(`/endpoints/${NAME}`)) return json(manifest);
    if (path.endsWith(`/${PROJECT}/endpoints`)) return json(list([manifest]));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR ?? join("test-results", "endpoint-open-data");
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("an open-data Endpoint (EP-187)", () => {
  for (const width of [1440, 400]) {
    test(`the read-only badge, the write-Policy warning and the editor's refusal at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, MARKED);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      await expect(page.getByText("Open data: read-only")).toBeVisible();
      const warning = page.getByRole("status").filter({ hasText: "This open-data Endpoint binds a Policy that writes" });
      await expect(warning).toContainText("Policy sensors-write grants updateAttrs");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      await shot(page, `endpoint-open-data-write-${width}`);

      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}/edit?lang=en`);
      const notice = page.getByRole("note").filter({ hasText: "open data and read-only for everyone" });
      await expect(notice).toBeVisible();
      await shot(page, `endpoint-edit-open-data-${width}`);
      await page.getByRole("button", { name: "Check", exact: true }).click();
      const refusal = page.getByRole("alert").filter({ hasText: "Policy sensors-write grants updateAttrs" });
      await expect(refusal).toBeVisible();
      await refusal.scrollIntoViewIfNeeded();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      await shot(page, `endpoint-edit-refused-${width}`);
    });

    test(`open data that only reads, and an internal Endpoint, at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, MARKED.slice(0, 1));
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      await expect(page.getByText("Open data: read-only")).toBeVisible();
      await expect(page.getByText("This open-data Endpoint binds a Policy that writes")).toHaveCount(0);
      await shot(page, `endpoint-open-data-${width}`);
      await page.unrouteAll({ behavior: "wait" });
      await stub(page, []);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      await expect(page.getByRole("heading", { name: "Air quality, open data" })).toBeVisible();
      await expect(page.getByText("Open data: read-only")).toHaveCount(0);
      await shot(page, `endpoint-internal-${width}`);
    });
  }
});
