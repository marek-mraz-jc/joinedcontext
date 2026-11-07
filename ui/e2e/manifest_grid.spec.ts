import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { readFileSync } from "node:fs";
import { axeViolations } from "./axe";

// T-6115: Endpoints, Subscriptions, Data models and Pipelines are one grid (owner 2026-10-29).
// Route stubs only: the shots show the grid at 1440 and 400, a filter chip, the cell panel with
// its schema widgets, the server's refusal on a field and the empty list.

const V = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: V, kind: "List", items });
const schema = (kind: string) => JSON.parse(readFileSync(new URL(`../tests/fixtures/kinds/${kind}.json`, import.meta.url), "utf8")) as unknown;

const endpoints = [
  ["air-quality", "Air quality", "air", "public", ["ngsi-ld", "ogc-features", "mcp"]],
  ["parking", "Parking occupancy", "mobility", "organization", ["ngsi-ld"]],
  ["street-works", "Street works", "works", "public", ["ngsi-ld", "file.csv"]],
  ["noise", "Noise sensors", "air", "project-list", ["ngsi-ld", "odata"]],
].map(([name, title, space, audience, enabledRepresentations]) => ({
  apiVersion: V,
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: space, slug: `${name}helsinkiendpointaaaaaaaa`.slice(0, 30), audience, enabledRepresentations },
  status: { phase: "Live" },
}));
const subscriptions = [
  { name: "air-alerts", space: "air", type: "AirQualityObserved", uri: "https://alerts.example.org/hook", active: true },
  { name: "parking-full", space: "mobility", type: "ParkingSpot", uri: "https://parking.example.org/full", active: false },
].map(({ name, space, type, uri, active }) => ({
  apiVersion: V,
  kind: "Subscription",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space, entities: [{ type }], isActive: active, throttling: 60, notification: { endpoint: { uri, accept: "application/json" } } },
  // A paused one was applied (Live) and stopped sending: fewer deliveries, the last one days ago.
  status: active
    ? { phase: "Live", notifications: { timesSent: 42, timesFailed: 0, lastSuccess: "2026-10-30T05:00:00Z" } }
    : { phase: "Live", notifications: { timesSent: 7, timesFailed: 0, lastSuccess: "2026-10-21T09:15:00Z" } },
}));
const models = [
  { name: "air", title: "Air quality", space: "air", version: "1.2.0", lifecycle: "published" },
  { name: "mobility", title: "Mobility", space: "mobility", version: "0.3.0", lifecycle: "draft" },
].map(({ name, title, space, version, lifecycle }) => ({
  apiVersion: V,
  kind: "DataModel",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: space, version, lifecycle, linkml: "id: x\nname: x\n", classes: ["AirQualityObserved"] },
  status: { phase: "Applied" },
}));
const pipelines = [
  { name: "air-intake", klass: "resident", enabled: true },
  { name: "works-daily", klass: "scheduled", enabled: false },
].map(({ name, klass, enabled }) => ({
  apiVersion: V,
  kind: "Pipeline",
  metadata: { name, namespace: "helsinki" },
  spec: { class: klass, enabled, period: "5m", targetEndpoint: "air-quality", output: { type: "AirQualityObserved" } },
  status: { phase: "Live" },
}));
// T-6169: an Endpoint espoo shares with the whole organization, a broker for the generic kind list.
const espooEndpoints = [
  {
    apiVersion: V,
    kind: "Endpoint",
    metadata: { name: "bikes", namespace: "espoo", title: { en: "City bikes in Espoo" } },
    spec: { contextSpaceRef: "liikenne", slug: "bikesespooendpointaaaaaaaaaaaa", audience: "organization", enabledRepresentations: ["ngsi-ld", "mcp"] },
    status: { phase: "Live" },
  },
];
const brokers = [
  { apiVersion: V, kind: "MqttBroker", metadata: { name: "sensors", namespace: "helsinki", title: { en: "Street sensors" } }, spec: { contextSpaceRef: "air" }, status: { phase: "Live" } },
  { apiVersion: V, kind: "MqttBroker", metadata: { name: "buses", namespace: "helsinki" }, spec: { contextSpaceRef: "mobility" }, status: { phase: "Pending" } },
];
const usage = {
  from: "2026-10-24",
  to: "2026-10-30",
  endpoints: [{ name: "air-quality", slug: "x", total: 18342, daily: [], totals: { "ngsi-ld": 15000, mcp: 3000, "ogc": 342 }, callers: { person: 4200, workload: 11000, agent: 3000, anonymous: 142 } }],
};
const spaces = ["air", "mobility", "works"].map((name) => ({ apiVersion: V, kind: "ContextSpace", metadata: { name, namespace: "helsinki" }, spec: {}, status: { phase: "Live" } }));

async function stub(page: Page, opts: { empty?: boolean; refuse?: boolean; slow?: boolean } = {}) {
  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Kováčová", roles: ["portal-editor"] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", bootstrap: false, grants: [{ scope: "project:helsinki", rule: { kinds: ["Endpoint", "Subscription", "DataModel", "Pipeline", "ContextSpace", "MqttBroker", "SharedSpaceReference"], verbs: ["read", "propose"] } }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: V, kind: "ProjectList", items: [{ name: "helsinki" }, { name: "espoo" }] });
    if (path === "/api/v1/projects/espoo/endpoints") return json(list(opts.empty ? [] : espooEndpoints));
    if (path.endsWith("/usage/endpoints")) return json(usage);
    const spaceUsage = /\/spaces\/(\w+)\/usage$/.exec(path)?.[1];
    if (spaceUsage) return json({ entities: spaceUsage.length * 1234, attrInstances: spaceUsage.length * 9876, bytes: { entities: 1048576, attrInstances: 4194304 }, observedAt: "2026-10-30T06:00:00Z" });
    if (opts.slow && /\/projects\/helsinki\/\w+$/.test(path)) await new Promise((resolve) => setTimeout(resolve, 60_000));
    const kind = /^\/api\/v1\/kinds\/(\w+)\/schema$/.exec(path)?.[1];
    if (kind) return json(schema(kind));
    if (request.method() !== "GET" && opts.refuse) {
      return json({ type: "about:blank", title: "Forbidden", status: 403, detail: "Your role may not change the audience of Endpoint air-quality: ask a project steward." }, 403);
    }
    const lists: Record<string, unknown[]> = { endpoints, subscriptions, datamodels: models, pipelines, spaces, mqttbrokers: brokers };
    // The cell panel reads the one row afresh before it writes (F-R2UI2-007).
    const one = /^\/api\/v1\/projects\/helsinki\/(\w+)\/([\w-]+)$/.exec(path);
    if (one && request.method() === "GET" && one[1] in lists) {
      const found = (lists[one[1]] as { metadata: { name: string } }[]).find((item) => item.metadata.name === one[2]);
      if (found) return json(found);
    }
    const plural = /^\/api\/v1\/projects\/helsinki\/(\w+)$/.exec(path)?.[1];
    if (plural && plural in lists) return json(list(opts.empty && plural !== "spaces" ? [] : lists[plural]));
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json(list([]));
  });
}

const PAGES = [
  ["endpoints", "Air quality"],
  ["subscriptions", "air-alerts"],
  ["models", "Air quality"],
  ["pipelines", "air-intake"],
  ["spaces", "air"],
  ["mqttbrokers", "Street sensors"],
] as const;

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    for (const [route, first] of PAGES) {
      test(`${route} is the shared grid`, async ({ page }, info) => {
        await stub(page);
        await page.goto(`/projects/helsinki/${route}?lang=en`);
        await expect(page.getByRole("link", { name: first }).first()).toBeVisible();
        await expect(page.getByRole("toolbar").first()).toBeVisible();
        await expect(page.getByRole("button", { name: /^More actions for / }).first()).toBeVisible();
        await page.screenshot({ path: info.outputPath(`${route}-filled-${width}.png`), fullPage: true });
        expect(await axeViolations(page)).toEqual([]);
      });
    }

    test("a shared Endpoint is a row with its Source, Use in this project and View YAML (T-6169)", async ({ page }, info) => {
      await stub(page);
      await page.goto("/projects/helsinki/endpoints?lang=en");
      const row = page.getByRole("row").filter({ hasText: "City bikes in Espoo" });
      await expect(row.getByText("Shared by espoo")).toBeVisible();
      if (width === 1440) await expect(page.getByRole("row").filter({ hasText: "Air quality" }).getByText("18,342")).toBeVisible();
      await row.getByRole("button", { name: /More actions/ }).click();
      await expect(page.getByRole("menuitem", { name: "Use in this project" })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`endpoints-shared-menu-${width}.png`) });
      await page.getByRole("menuitem", { name: "View YAML" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByText("kind: Endpoint")).toBeVisible();
      await expect(dialog.getByText("status")).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(`endpoints-yaml-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
    });

    test("a list on its way shows a skeleton that fits the screen", async ({ page }, info) => {
      await stub(page, { slow: true });
      await page.goto("/projects/helsinki/mqttbrokers?lang=en");
      await expect(page.locator("tbody[aria-hidden=true]")).toBeVisible();
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(sideways, "the page scrolls sideways while loading").toBeLessThanOrEqual(1);
      await page.screenshot({ path: info.outputPath(`mqttbrokers-loading-${width}.png`) });
    });

    test("a filter chip narrows the endpoints and the panel edits the audience", async ({ page }, info) => {
      await stub(page);
      await page.goto("/projects/helsinki/endpoints?lang=en");
      await page.getByText("Filter", { exact: true }).click();
      await page.getByRole("checkbox", { name: /^Organization/ }).check();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("button", { name: "Remove filter Audience: Organization" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Air quality" })).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(`endpoints-filtered-${width}.png`), fullPage: true });
      await page.getByRole("button", { name: "Remove filter Audience: Organization" }).click();
      await page.getByRole("button", { name: "More actions for Air quality" }).click();
      await page.getByRole("menuitem", { name: "Details" }).click();
      const panel = page.getByRole("dialog", { name: "Air quality" });
      await expect(panel.getByRole("combobox", { name: "Audience" })).toHaveValue("public");
      await expect(panel.getByRole("group", { name: "Representations" })).toBeVisible();
      // T-6186: the 7-day calls split by kind of caller, shown in the panel, never who called.
      await expect(panel.getByRole("list", { name: "By caller" })).toContainText("Apps and workloads 11,000");
      await panel.getByRole("list", { name: "By caller" }).scrollIntoViewIfNeeded();
      await page.screenshot({ path: info.outputPath(`endpoints-panel-calls-${width}.png`) });
      await page.screenshot({ path: info.outputPath(`endpoints-panel-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
    });

    test("the panel says in words when an Endpoint had no calls or is counted elsewhere (T-6186)", async ({ page }, info) => {
      await stub(page);
      await page.goto("/projects/helsinki/endpoints?lang=en");
      await page.getByRole("button", { name: "More actions for Parking occupancy" }).click();
      await page.getByRole("menuitem", { name: "Details" }).click();
      const zero = page.getByRole("dialog", { name: "Parking occupancy" });
      await zero.getByText("No answered calls in the last 7 days").scrollIntoViewIfNeeded();
      await expect(zero.getByText("No answered calls in the last 7 days")).toBeVisible();
      await page.screenshot({ path: info.outputPath(`endpoints-panel-calls-zero-${width}.png`) });
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "More actions for espoo/City bikes in Espoo" }).click();
      await page.getByRole("menuitem", { name: "Details" }).click();
      const shared = page.getByRole("dialog", { name: /City bikes in Espoo/ });
      const reason = shared.getByText("Usage is counted by the project that publishes this Endpoint.");
      await reason.scrollIntoViewIfNeeded();
      await expect(reason).toBeVisible();
      await page.screenshot({ path: info.outputPath(`endpoints-panel-calls-shared-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
    });

    test("the server's refusal shows on the field", async ({ page }, info) => {
      await stub(page, { refuse: true });
      await page.goto("/projects/helsinki/endpoints?lang=en");
      await page.getByRole("button", { name: "More actions for Air quality" }).click();
      await page.getByRole("menuitem", { name: "Details" }).click();
      const panel = page.getByRole("dialog", { name: "Air quality" });
      await panel.getByRole("combobox", { name: "Audience" }).selectOption("organization");
      await panel.getByRole("button", { name: "Propose change" }).click();
      await expect(panel.getByText(/may not change the audience/)).toBeVisible();
      await page.screenshot({ path: info.outputPath(`endpoints-refused-${width}.png`) });
    });

    test("an empty project says what to do next", async ({ page }, info) => {
      await stub(page, { empty: true });
      await page.goto("/projects/helsinki/subscriptions?lang=en");
      await expect(page.getByRole("button", { name: "New Subscription" }).first()).toBeVisible();
      await page.screenshot({ path: info.outputPath(`subscriptions-empty-${width}.png`), fullPage: true });
    });
  });
}
