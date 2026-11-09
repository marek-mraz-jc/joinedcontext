import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// jc-ui-tables (UI judge on the prod rehearsal, 2026-11-09): the tables of these pages were cut
// off at 400 px and named people by their Keycloak ids. Each page in each state, at 1440 and
// 400 px: the page never scrolls sideways, no person id is printed, and a shot is kept in
// test-results/jc-ui-tables for the judge.

const V = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: V, kind: "List", items });
const STRANGER = "fc3b55df-5fd4-4679-af5e-bb9dca22b0d7";
const MEMBER = "de300000-0000-4000-8000-000000000001";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };

type State = "filled" | "empty" | "error" | "refusal";

const ALL = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["*", "RoleBinding", "Organization", "Group"], verbs: ["read", "propose", "approve", "delete"] } }],
};
const NONE = { project: "org", bootstrap: false, grants: [] };

const BINDINGS = [
  { apiVersion: V, kind: "RoleBinding", metadata: { name: "admins", namespace: "org" }, spec: { subjects: [{ user: STRANGER }], role: "org-admin", scope: { organization: "hel" } } },
  { apiVersion: V, kind: "RoleBinding", metadata: { name: "editors", namespace: "org" }, spec: { subjects: [], role: "editor", scope: { organization: "hel" } } },
  { apiVersion: V, kind: "RoleBinding", metadata: { name: "barcelona-viewers", namespace: "org" }, spec: { subjects: [{ group: "barcelona" }], role: "viewer", scope: { organization: "hel" } } },
  { apiVersion: V, kind: "RoleBinding", metadata: { name: "stewards", namespace: "org" }, spec: { subjects: [{ user: "demo.steward@hel.fi" }], role: "steward", scope: { project: "helsinki" } } },
];

const GROUPS = [
  {
    apiVersion: V,
    kind: "Group",
    metadata: { name: "air-quality-steward", namespace: "org" },
    spec: { description: "The stewards of air-quality, its default group.", members: [{ user: MEMBER }] },
    status: { phase: "Live", conditions: [{ type: "GroupSynced", status: "False", reason: "MemberUnknown", message: `${MEMBER} belongs in the group and the realm has no user with that id` }] },
  },
  {
    apiVersion: V,
    kind: "Group",
    metadata: { name: "bike-weather-demand-viewer", namespace: "org" },
    spec: { description: "The viewers of bike-weather-demand, its default group.", members: [{ user: STRANGER }, { user: "lead@hel.fi" }] },
    status: { phase: "Live", conditions: [{ type: "GroupSynced", status: "True", reason: "DriftCorrected", message: `${STRANGER} belongs in the group and was added` }] },
  },
];

const POLICIES = [
  { apiVersion: V, kind: "Policy", metadata: { name: "app-banskabystrica-data-1", namespace: "helsinki", title: { en: "Data of Banská Bystrica" } }, spec: { contextSpaceRef: "banskabystrica-verejne", assigner: "did:web:hel.fi", assignee: { kind: "user", id: STRANGER }, operations: ["queryEntity", "retrieveEntity"] }, status: { phase: "Live" } },
  { apiVersion: V, kind: "Policy", metadata: { name: "air-team-reads", namespace: "helsinki" }, spec: { contextSpaceRef: "ovzdusie", assigner: "did:web:hel.fi", assignee: { kind: "group", id: "air-team" }, operations: ["retrieveOps", "updateAttrs"] }, status: { phase: "Live" } },
];

const SOURCES = [
  { apiVersion: V, kind: "DataSource", metadata: { name: "mqtt-mesto", namespace: "helsinki", title: { en: "City MQTT broker" } }, spec: { type: "mqtt", endpoint: "mqtts://mqtt.banskabystrica.sk:8883/sensors/air/#", secretRef: { name: "mqtt-mesto" } } },
  { apiVersion: V, kind: "DataSource", metadata: { name: "kataster-wfs", namespace: "helsinki" }, spec: { type: "http", endpoint: "https://kataster.skgeodesy.sk/eskn/rest/services/INSPIRE/CP_WFS/MapServer/WFSServer?service=WFS&request=GetFeature" } },
];

const ACCOUNTS = [
  { apiVersion: V, kind: "ServiceAccount", metadata: { name: "air-sensors", namespace: "helsinki", title: { en: "Air sensors" } }, spec: { owner: { user: STRANGER }, purpose: "Writes the air sensors' readings." }, status: { phase: "Live" } },
];

const LIMITS = {
  entries: [{ path: "spec.projects.quota.contextSpaces", section: "projects", security: false, default: null, min: 0, max: 20, value: 10, origin: "organization" }],
  projects: ["banskabystrica", "barcelona", "zilina"].map((project) => ({
    project,
    origin: "organization",
    quota: { contextSpaces: { limit: 10, used: 3 }, endpoints: { limit: 20, used: 7 }, publicEndpoints: { limit: 5, used: 1 }, pipelines: { limit: 30, used: 12 }, agentRunsPerDay: { limit: null, used: null } },
  })),
};

const ACTIVITY = [
  { time: "2026-11-09T07:52:00Z", project: "helsinki", kind: "config.drift", source: "reconciler", severity: "error", summary: "The repository did not load: git error: git transport error: error sending request for url (http://127.0.0.1:42102/odata/Repositories(Owner='joinedcontext',Name='helsinki-config')/git/info/refs?service=git-upload-pack)" },
  { time: "2026-11-09T07:11:39Z", project: "helsinki", kind: "mcp.tool", source: "portal", severity: "warning", summary: "smoke-mcp-organization-935217 refused jc_person_list through mcp-helsinki-organization-connector-with-a-long-name" },
  { time: "2026-11-09T07:11:36Z", project: "helsinki", kind: "config.applied", source: "reconciler", severity: "info", summary: "The repository at 55ab26b is live: 3443 manifests." },
];

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ENDPOINT = { apiVersion: V, kind: "Endpoint", metadata: { name: "app-banskabystrica-data", namespace: "helsinki", title: { en: "Data of Banská Bystrica" } }, spec: { contextSpaceRef: "banskabystrica-verejne", slug: SLUG, audience: "public" } };
const ATTRS = ["address", "annualBudget", "budgetYear", "category", "location", "name", "pm10", "pm25", "pupils", "schoolCode", "stationCode", "teachingLanguage"];
const GRANTS = {
  subject: { type: "role", id: "public" },
  resource: { type: "endpoint", id: SLUG, space: "helsinki" },
  permissions: ["Event", "Organization", "AirQualityObserved"].map((type) => ({ resource: { type }, actions: ["retrieveEntity", "queryEntity"], attributes: ATTRS, constraints: { scopeQ: "/helsinki/#" } })),
  prohibitions: [],
};

const PROBLEM = { type: "https://joinedcontext.com/errors/unavailable", title: "Service Unavailable", status: 503, detail: "The Portal could not read its repository just now." };
const FORBIDDEN = { type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: "You may not read this list." };

/** Every list the pages read, answered for one state. */
async function stub(page: Page, state: State): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/branding")) return json({ instanceName: "joinedcontext", hiddenSections: ["dashboards"] });
    if (path.endsWith("/permissions/me")) return json(state === "refusal" ? NONE : ALL);
    if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
    if (path.endsWith("/activity/stream")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    const rows: Record<string, unknown[]> = {
      "/api/v1/projects/org/rolebindings": BINDINGS,
      "/api/v1/projects/org/groups": GROUPS,
      "/api/v1/projects/helsinki/policies": POLICIES,
      "/api/v1/projects/helsinki/datasources": SOURCES,
      "/api/v1/projects/helsinki/serviceaccounts": ACCOUNTS,
      "/api/v1/projects/helsinki/endpoints": [ENDPOINT],
    };
    if (path in rows || path.endsWith("/activity") || path === "/api/v1/organization/limits") {
      if (state === "error") return json(PROBLEM, 503);
      if (state === "refusal" && !path.endsWith("/endpoints")) return json(FORBIDDEN, 403);
      if (path.endsWith("/activity")) return json({ apiVersion: V, kind: "ActivityList", items: state === "empty" ? [] : ACTIVITY });
      if (path === "/api/v1/organization/limits") return json(state === "empty" ? { entries: [], projects: [] } : LIMITS);
      return json(list(state === "empty" ? [] : rows[path]));
    }
    if (path.includes(`/api/endpoint/${SLUG}`)) return json(state === "filled" ? GRANTS : { ...GRANTS, permissions: [] });
    return json(list([]));
  });
}

/** What the filled state shows: a table, or the page's own words where it has none. */
const PAGES: { name: string; address: string; shows?: string }[] = [
  { name: "organization-members", address: "/organization/members" },
  { name: "organization-groups", address: "/organization/groups" },
  { name: "organization-settings", address: "/organization/settings" },
  { name: "policies", address: "/projects/helsinki/policies" },
  { name: "datasources", address: "/projects/helsinki/datasources" },
  { name: "settings-access", address: "/projects/helsinki/settings/access" },
  { name: "settings-service-accounts", address: "/projects/helsinki/settings/service-accounts", shows: "Air sensors" },
  { name: "activity", address: "/projects/helsinki/activity" },
  { name: "dashboards", address: "/projects/helsinki/dashboards", shows: "Dashboards are not part of this installation" },
];

for (const { name, address, shows } of PAGES) {
  for (const state of ["filled", "empty", "error", "refusal"] as const) {
    for (const width of [1440, 400]) {
      test(`${name} ${state} at ${width} px`, async ({ page }) => {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await stub(page, state);
        await page.goto(`${address}?lang=en`);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
        await page.waitForLoadState("networkidle");
        // The filled state shows its rows: a page that refused the admin would pass the rest.
        if (state === "filled") {
          await expect(shows ? page.getByText(shows).first() : page.locator("main table").first()).toBeVisible();
        }
        // The page itself never scrolls sideways; a table that must is its own frame.
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        expect(overflow).toBeLessThanOrEqual(0);
        const text = (await page.locator("main").innerText()).toLowerCase();
        expect(text).not.toContain(STRANGER);
        expect(text).not.toContain(MEMBER);
        await page.screenshot({ path: `test-results/jc-ui-tables/${name}-${state}-${width}.png`, fullPage: true });
      });
    }
  }
}
