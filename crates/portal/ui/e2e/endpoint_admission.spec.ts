import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { attributeTable } from "./attributeAccess";

// T-6802: an administrator outside an Endpoint's project groups opens its page and
// settings/access; the Portal's admission answer says the gateway refuses them, the pages show the
// refused state and ask nothing under /api/endpoint/{slug}. Admitted, both read as before.
// `vite preview` has no Portal behind it: the API is answered in the browser.

const PROJECT = "helsinki";
const NAME = "app-air-quality";
const SLUG = "3jlwl5dg2oidblwqhc6qdi3zxl";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT, title: "Air quality app" },
  spec: { contextSpaceRef: "air", slug: SLUG, audience: "project-list", enabledRepresentations: ["ngsi-ld", "mcp"] },
  status: { phase: "Live" },
};
const ACCESS = {
  subject: { type: "user", id: "smoke-ui-reader" },
  resource: { type: "endpoint", id: SLUG, space: "air" },
  permissions: [{ resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*", constraints: {} }],
  prohibitions: [],
};

const SCHEMA = {
  $defs: {
    AirQualityObserved: {
      properties: {
        name: { type: "string", description: "Where the station stands" },
        pm10: { type: "number", description: "PM10 in µg/m³, the hourly mean" },
        dateObserved: { type: "string", format: "date-time", description: "When the hour ended" },
      },
    },
  },
};
const STATIONS = ["Kallio", "Mannerheimintie", "Mäkelänkatu", "Töölö", "Vartiokylä"];
const ROWS = STATIONS.map((station, n) => ({
  id: `urn:ngsi-ld:AirQualityObserved:helsinki-${n}`,
  type: "AirQualityObserved",
  name: { type: "Property", value: station },
  pm10: { type: "Property", value: 12 + n * 7 },
  dateObserved: { type: "Property", value: new Date(Date.UTC(2026, 10, 12, n)).toISOString() },
}));
const CARD = "Query AirQualityObserved through this Endpoint: GET /ngsi-ld/v1/entities?type=AirQualityObserved";
const day = (n: number) => new Date(Date.UTC(2026, 9, 14 + n)).toISOString().slice(0, 10);
const USAGE = {
  name: NAME,
  from: day(0),
  to: day(29),
  days: Array.from({ length: 30 }, (_, n) => ({ day: day(n), calls: { "ngsi-ld": 120 + n * 3, mcp: 8 }, total: 128 + n * 3 })),
  totals: { "ngsi-ld": 4905, mcp: 240 },
  total: 5145,
};
const QUALITY = {
  score: 96,
  measuredAt: "2026-11-12T03:00:00Z",
  parts: [
    { part: "freshness", score: 100, measuredAt: "2026-11-12T03:00:00Z", reason: { code: "onTime", args: { fresh: "1", total: "1" }, text: "1 of 1 feeds are on schedule" } },
    { part: "completeness", score: 97, measuredAt: "2026-11-12T03:00:00Z", reason: { code: "complete", args: { complete: "1", fields: "2" }, text: "1 of 2 required fields are filled in every entity" } },
    { part: "validity", score: 95, measuredAt: "2026-11-12T03:00:00Z", reason: { code: "valid", args: { valid: "190", checked: "200" }, text: "190 of 200 entities follow the model" } },
    { part: "links", score: 100, measuredAt: "2026-11-12T03:00:00Z", reason: { code: "linksUp", args: { up: "2", total: "2" }, text: "2 of 2 access links answer" } },
  ],
};

// Who reads it (ADR-N-067 §2.5): the project's readers group, by its Policy.
const READERS = {
  columns: [{ kind: "organization" }, { kind: "group", group: "helsinki-readers" }],
  rows: [{
    project: PROJECT, name: NAME, slug: SLUG, title: "Air quality app", audience: "project-list", organization: "helsinki",
    cells: [
      { granted: false, rules: [], throughOrganization: false, unanswered: false },
      { granted: true, rules: [{ policy: "helsinki-readers-air", rule: 0, types: ["AirQualityObserved"] }], throughOrganization: false, unanswered: false },
    ],
  }],
  unanswered: 0,
};
const MATRIX = {
  endpoint: NAME,
  space: "air",
  principals: [{ kind: "group", name: "helsinki-readers", via: [{ kind: "policy", name: "helsinki-readers-air" }] }],
  types: ["AirQualityObserved"],
  cells: [{ principal: "group:helsinki-readers", type: "AirQualityObserved", action: "read", effect: "permit", attributes: "*", constraint: null, decidedBy: "helsinki-readers-air" }],
  edges: [
    { from: "principal:group:helsinki-readers", to: "policy:helsinki-readers-air" },
    { from: "policy:helsinki-readers-air", to: `endpoint:${NAME}` },
    { from: `endpoint:${NAME}`, to: "type:AirQualityObserved" },
  ],
};

/** Answers the page as the Portal and the gateway would; returns the gateway paths it was asked. */
async function stub(page: Page, admitted: boolean): Promise<string[]> {
  const gateway: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.startsWith("/api/endpoint/")) {
      gateway.push(path);
      if (!admitted) return json({ title: "Forbidden", status: 403 }, 403);
      if (path.endsWith("/access")) return json(ACCESS);
      if (path.endsWith("/access/attributes")) return json(attributeTable("AirQualityObserved", ["name", "pm10", "dateObserved"]));
      if (path.endsWith("/schema/index.json")) return json({ models: [{ name: "air", version: 1 }] });
      if (path.endsWith("/schema/v1/json-schema")) return json(SCHEMA);
      if (path.endsWith("/ngsi-ld/v1/entities"))
        return route.fulfill({ contentType: "application/json", headers: { "NGSILD-Results-Count": String(ROWS.length) }, body: JSON.stringify(ROWS) });
      if (path.endsWith("/mcp")) return json({ jsonrpc: "2.0", id: 1, result: { contents: [{ uri: `card://${SLUG}`, text: CARD }] } });
      return json({}, 404);
    }
    if (path.endsWith("/auth/me"))
      return json({ subject: "s", username: "smoke-ui-admin", name: "Smoke Admin", roles: ["portal-admin"] });
    if (path === "/api/v1/projects") return json({ items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith(`/endpoints/${NAME}/admission`)) return json({ admitted });
    if (path.endsWith(`/endpoints/${NAME}/usage`)) return json(USAGE);
    if (path.endsWith(`/endpoints/${NAME}/quality`)) return json(QUALITY);
    if (path.endsWith(`/endpoints/${NAME}/access/matrix`)) return json(MATRIX);
    if (path === "/api/v1/organization/data-access") return json(READERS);
    if (path.endsWith(`/endpoints/${NAME}`)) return json(ENDPOINT);
    if (path.endsWith(`/${PROJECT}/endpoints`)) return json(list([ENDPOINT]));
    if (path.endsWith(`/endpoints/${NAME}/access-requests`)) return json({ types: ["AirQualityObserved"], refusal: null, items: [] });
    return json(list([]));
  });
  return gateway;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("an Endpoint the gateway does not admit you to (T-6802)", () => {
  for (const width of [1440, 400]) {
    test(`the refused state, no gateway read, at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const gateway = await stub(page, false);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      const refused = page.locator("[data-endpoint-refused]");
      await expect(refused.getByText("You do not read this Endpoint's data")).toBeVisible();
      await refused.scrollIntoViewIfNeeded();
      await shot(page, `endpoint-refused-${width}`);
      await refused.getByRole("link", { name: "Request access" }).click();
      await expect(page).toHaveURL(/#request-access$/);

      await page.goto(`/projects/${PROJECT}/settings/access?lang=en`);
      await expect(page.locator("[data-endpoint-refused]").getByText("You do not read this Endpoint's data")).toBeVisible();
      await shot(page, `settings-access-refused-${width}`);
      await page.locator("[data-endpoint-refused]").getByRole("link", { name: "Request access" }).click();
      await expect(page).toHaveURL(new RegExp(`/endpoints/${NAME}#request-access$`));
      expect(gateway).toEqual([]);
    });

    test(`an admitted reader reads as before at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const gateway = await stub(page, true);
      await page.goto(`/projects/${PROJECT}/endpoints/${NAME}?lang=en`);
      await expect.poll(() => gateway.length).toBeGreaterThan(0);
      await expect(page.locator("[data-endpoint-refused]")).toHaveCount(0);
      const data = page.locator("section", { has: page.getByRole("heading", { name: "Data", exact: true }) });
      await expect(data.getByText("Mannerheimintie").first()).toBeVisible();
      await expect(page.getByText(CARD)).toBeVisible();
      await shot(page, `endpoint-granted-${width}`);

      await page.goto(`/projects/${PROJECT}/settings/access?lang=en`);
      await expect.poll(() => gateway).toContain(`/api/endpoint/${SLUG}/access`);
      await expect(page.locator("[data-endpoint-refused]")).toHaveCount(0);
      await shot(page, `settings-access-granted-${width}`);
    });
  }
});
