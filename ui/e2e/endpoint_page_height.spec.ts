import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// T-5015: app-air-weather-explorer's page on dev was 23,431 px at 1440. Its filter proof listed 50
// entities with every attribute, and "Who may call it" printed each of the space's 40 policies with
// one line per operation. The proof now pages 12 entities, single operations share a row and the
// policies fold after five and the proof's frame scrolls: the page stays under 6,000 px at 1440
// (9,000 at 400) with the same data. T-5118 added the Usage section (about 420 px of chart and
// totals at 1440): the 1440 budget is 6,600 since, still a quarter of what dev showed.

const API = "joinedcontext.com/v1alpha1";
const SLOTS = ["address", "description", "endDate", "eventStatus", "location", "name", "source", "startDate"];

const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "events", namespace: "helsinki", labels: { "joinedcontext.com/space": "tapahtumat" } },
  spec: { contextSpaceRef: "tapahtumat", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "project-list", projectionRef: "events", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};
const PROJECTION = {
  apiVersion: API,
  kind: "ModelProjection",
  metadata: { name: "events", namespace: "helsinki" },
  spec: { contextSpaceRef: "tapahtumat", classes: [{ name: "Event", slots: SLOTS }] },
};
const SPACE = { apiVersion: API, kind: "ContextSpace", metadata: { name: "tapahtumat", namespace: "helsinki" }, spec: {} };
const POLICIES = Array.from({ length: 40 }, (_, n) => ({
  apiVersion: API,
  kind: "Policy",
  metadata: { name: `public-read-${n}`, namespace: "helsinki", title: { en: `Public read ${n}` } },
  spec: {
    contextSpaceRef: "tapahtumat",
    assignee: { kind: "role", id: "public" },
    operations: ["retrieveOps", "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveAttrTypes", "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"],
  },
}));
const EVENTS = Array.from({ length: 120 }, (_, n) => ({
  id: `urn:ngsi-ld:Event:helsinki-${n}`,
  type: "Event",
  address: { type: "Property", value: "Aleksanterinkatu 16, Helsinki" },
  description: { type: "Property", value: "Travel back in time to the Helsinki of the past, with a guide." },
  endDate: { type: "Property", value: "2026-09-14T07:00:00Z" },
  eventStatus: { type: "Property", value: "EventScheduled" },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.95, 60.17] } },
  name: { type: "Property", value: `Time machine ${n}` },
  source: { type: "Property", value: "https://api.hel.fi/linkedevents/v1/" },
  startDate: { type: "Property", value: "2020-11-11T09:00:00Z" },
}));
// A busy Endpoint is read every day (T-5118): its Usage section draws the month, not a placeholder.
const day = (n: number) => new Date(Date.UTC(2026, 8, 23 + n)).toISOString().slice(0, 10);
const USAGE = {
  name: "events",
  from: day(0),
  to: day(29),
  days: Array.from({ length: 30 }, (_, n) => ({ day: day(n), calls: { "ngsi-ld": 400 + n, mcp: 20 }, total: 420 + n })),
  totals: { "ngsi-ld": 12435, mcp: 600 },
  total: 13035,
};

for (const width of [1440, 400]) {
  test(`a busy Endpoint's page stays short at ${width} px (T-5015)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) {
        return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
      }
      if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
      if (path === "/api/v1/projects/helsinki/endpoints/events") return json(ENDPOINT);
      if (path === "/api/v1/projects/helsinki/endpoints/events/preview") {
        const asked = route.request().postDataJSON() as { id?: string[]; limit: number };
        const served = asked.id ? EVENTS.filter((one) => asked.id?.includes(one.id)) : EVENTS;
        return json({ count: EVENTS.length, entities: served.slice(0, asked.limit) });
      }
      if (path.endsWith("/projections")) return json(list([PROJECTION]));
      if (path.endsWith("/spaces")) return json(list([SPACE]));
      if (path.endsWith("/policies")) return json(list(POLICIES));
      if (path === "/api/v1/projects/helsinki/endpoints/events/usage") return json(USAGE);
      return json(list([]));
    });
    // The space's canonical surface, read with the person's rights, answers one page of the events.
    await page.route("**/cs/**", (route) => {
      const query = new URL(route.request().url()).searchParams;
      const offset = Number(query.get("offset") ?? 0);
      const limit = Number(query.get("limit") ?? 0);
      return route.fulfill({
        contentType: "application/ld+json",
        headers: { "NGSILD-Results-Count": String(EVENTS.length) },
        body: JSON.stringify(EVENTS.slice(offset, offset + limit)),
      });
    });
    await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));

    await page.goto("/projects/helsinki/endpoints/events?lang=en");
    await expect(page.getByTestId("proof-row")).toHaveCount(12);
    const grants = page.locator("section", { has: page.getByRole("heading", { name: "Who may call it" }) });
    await expect(grants.getByRole("listitem")).toHaveCount(5);
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    // A phone stacks every fact in one column: its gate is wider, still a third of what dev showed.
    expect(height).toBeLessThan(width === 1440 ? 6600 : 9000);

    const dir = join("test-results", "T-5015");
    mkdirSync(dir, { recursive: true });
    await page.screenshot({ path: join(dir, `endpoint-folded-${width}.png`), fullPage: true });
    await grants.getByRole("button", { name: "Show all 40 policies" }).click();
    await expect(grants.getByRole("listitem")).toHaveCount(40);
    await grants.screenshot({ path: join(dir, `grants-all-${width}.png`) });

    // A bare project address opens the project's home instead of the not-found page.
    await page.goto("/projects/helsinki?lang=en");
    await expect(page).toHaveURL(/\/projects\/helsinki\/home$/);
    await page.screenshot({ path: join(dir, `bare-project-${width}.png`) });
  });
}
