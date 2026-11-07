import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Your access, Data access table (T-5593): opened from an Endpoint's page on that Endpoint, each
// filter in words, two grants on one type grouped and each named by its Policy, at 1440 and 400 px.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.

const IDENTITY = { subject: "s", username: "demo.editor", email: "demo.editor@hel.fi", roles: ["portal-viewer"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const KINDS = ["ContextSpace", "DataModel", "Mapping", "Endpoint", "Pipeline", "DataSource", "Policy", "Dashboard"];
const GRANTS = [
  { role: "editor", rule: { verbs: ["propose", "approve"], kinds: KINDS }, binding: "editors", scope: "project:helsinki" },
  { role: "viewer", rule: { verbs: ["read"], kinds: ["*"] }, binding: "viewers", scope: "organization" },
];

const endpoint = (name: string, title: string, slug: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { slug, contextSpaceRef: "ilmanlaatu", audience: "organization" },
  status: { phase: "Live" },
});
const ENDPOINTS = [
  endpoint("air-quality", "Air quality", "mluyob4nz52lok3ssk7pgn5vwt"),
  endpoint("parking", "Parking", "zt4qm7ge2xdv6ksb3ncf5arw2y"),
];

const POLICY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name: "parking-last-day", namespace: "helsinki" },
  spec: {
    contextSpaceRef: "ilmanlaatu",
    assigner: "did:web:hel.fi",
    assignee: { kind: "role", id: "viewer" },
    operations: ["queryTemporal", "retrieveTemporal"],
    information: [{ entities: [{ type: "ParkingSpot" }], propertyNames: ["status", "occupancyModified"] }],
    temporalQ: "timerel=after;timeAt=P-1D",
  },
  status: { phase: "Live" },
};

const ACCESS = {
  subject: { type: "user", id: "demo.editor@hel.fi" },
  resource: { type: "endpoint", id: "zt4qm7ge2xdv6ksb3ncf5arw2y", space: "ilmanlaatu" },
  permissions: [
    { resource: { type: "ParkingSpot" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*", constraints: { q: "status==\"free\"" } },
    {
      resource: { type: "ParkingSpot" },
      actions: ["queryTemporal", "retrieveTemporal"],
      attributes: ["status", "occupancyModified"],
      constraints: { temporalQ: "timerel=after;timeAt=P-1D" },
    },
    { resource: { type: "ParkingGroup" }, actions: ["queryEntity"], attributes: "*", constraints: { scopeQ: "/helsinki/#" } },
  ],
  prohibitions: [],
};

async function stub(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: GRANTS });
    if (path === "/api/v1/projects/helsinki/endpoints") return json(list(ENDPOINTS));
    if (path === "/api/v1/projects/helsinki/endpoints/parking") return json(ENDPOINTS[1]);
    if (path === "/api/v1/projects/helsinki/policies") return json(list([POLICY]));
    if (path === "/api/endpoint/zt4qm7ge2xdv6ksb3ncf5arw2y/access") return json(ACCESS);
    if (path.startsWith("/api/endpoint/")) return route.fulfill({ status: 404, body: "" });
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("Your access opened from an Endpoint (T-5593)", () => {
  for (const width of [1440, 400]) {
    test(`an editor reads every grant of the Endpoint they came from at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page);

      await page.goto("/projects/helsinki/endpoints/parking?lang=en");
      await page.getByRole("link", { name: "Your access here" }).click();
      await expect(page).toHaveURL(/\/projects\/helsinki\/settings\/access\?endpoint=parking$/);
      await expect(page.getByLabel("Endpoint", { exact: true })).toHaveValue("zt4qm7ge2xdv6ksb3ncf5arw2y");

      const table = page.getByRole("table", { name: "Data access" });
      await expect(table.getByText("2 grants")).toBeVisible();
      await expect(table.getByText("history up to 1 day back")).toBeVisible();
      await expect(table.getByText('where status = "free"')).toBeVisible();
      await expect(table.getByText("within the scope /helsinki and everything under it")).toBeVisible();
      // The NGSI-LD text is only the hover title and the screen reader's aside.
      await expect(table.getByText("timerel=after;timeAt=P-1D", { exact: true })).toHaveCount(0);
      await expect(table.getByRole("link", { name: "parking-last-day" })).toBeVisible();
      await expect(page.getByText(/your own changes wait for another approver/).first()).toBeVisible();
      await shot(page, `your-access-${width}`);

      await page.getByLabel("Endpoint", { exact: true }).selectOption("mluyob4nz52lok3ssk7pgn5vwt");
      await expect(page).toHaveURL(/\?endpoint=air-quality$/);
    });
  }
});
