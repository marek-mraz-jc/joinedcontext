import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { dirname, join } from "node:path";
import { mkdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { pageFindings } from "../tests/pageChecks";
import { axeViolations } from "./axe";

// The project's daiya dashboards drawn in the Portal (item 15), on the built bundle with the API
// answered from the browser. The context-broker and helsinki dashboards are the deployment's own
// JSON (tests/fixtures/dashboards, deployment cee6ebcb9), stripped of their queries the way the
// Portal's dashboard route strips them; a third dashboard holds one panel of every other type.
// DASHBOARD_SHOTS=<dir> saves a page shot per state and a shot of each panel type per state.

const here = dirname(fileURLToPath(import.meta.url));
const NAMESPACES = Object.keys(JSON.parse(readFileSync(join(here, "../src/locales/en.json"), "utf8")) as object);
const deployed = (name: string) => JSON.parse(readFileSync(join(here, `../tests/fixtures/dashboards/${name}.json`), "utf8")) as Record<string, unknown>;

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: ["portal-approver"] };
const MIN = 60_000;
const HOUR = 60 * MIN;

type Field = { name: string; type: string; values: unknown[]; labels?: Record<string, string>; unit?: string; displayName?: string };
type Frame = { name: string; refId: string; fields: Field[] };

/** A wave a little different per series, so lines do not overlap. */
const wave = (n: number, base: number, swing: number, seed: number) => Array.from({ length: n }, (_, i) => +(base + swing * Math.sin(i / 3 + seed) + (swing / 3) * Math.cos(i / 1.7 + seed * 2)).toFixed(3));
const times = (n: number, span: number) => Array.from({ length: n }, (_, i) => Date.now() - span + (i * span) / (n - 1));
const series = (name: string, values: number[], span: number, refId = "A", labels?: Record<string, string>): Frame => ({
  name,
  refId,
  fields: [
    { name: "Time", type: "time", values: times(values.length, span) },
    { name: "Value", type: "number", values, ...(labels ? { labels } : { displayName: name }) },
  ],
});

/** What a panel's datasource would answer, per dashboard and panel. */
function filled(uid: string, id: number): Frame[] {
  const S = 6 * HOUR;
  if (uid === "jc-context-broker") {
    switch (id) {
      case 1:
        return [series("", wave(36, 42, 8, 1), S, "A", { status: "200" }), series("", wave(36, 6, 2, 2), S, "A", { status: "404" }), series("", wave(36, 0.4, 0.3, 3), S, "A", { status: "500" })];
      case 2:
        return [series("", wave(36, 30, 6, 4), S, "A", { method: "GET" }), series("", wave(36, 12, 3, 5), S, "A", { method: "POST" }), series("", wave(36, 4, 1, 6), S, "A", { method: "PATCH" })];
      case 3:
        return [series("p50", wave(36, 0.012, 0.003, 1), S, "A"), series("p95", wave(36, 0.085, 0.02, 2), S, "B"), series("p99", wave(36, 0.21, 0.06, 3), S, "C")];
      case 4:
        return [series("sent http", wave(36, 3.2, 0.8, 1), S, "A"), series("failed http", wave(36, 0.05, 0.05, 2), S, "B")];
      case 5:
        return [series("p95", wave(36, 0.004, 0.0015, 4), S)];
      case 6:
        return [series("timeouts", wave(36, 0.01, 0.01, 5).map((v) => Math.max(0, v)), S)];
      default:
        return [series("", wave(36, 3.1e8, 2e7, 7), S, "A", { pod: "antares-0" })];
    }
  }
  if (uid === "project-helsinki") {
    const W = 7 * 24 * HOUR;
    switch (id) {
      case 1:
        return ["Baana", "Eläintarhanlahti", "Kulosaaren silta", "Lauttasaaren silta"].map((name, i) => series(name, wave(7, 1800 - i * 300, 400, i).map(Math.round), W));
      case 2:
        return ["Kallio 2", "Mäkelänkatu", "Vartiokylä Huivipolku"].map((name, i) => series(name, wave(28, 6 + i * 3, 2.5, i), W));
      case 3:
        return [
          {
            name: "AirQualityObserved",
            refId: "A",
            fields: [
              { name: "Station", type: "string", values: ["Mäkelänkatu", "Kallio 2", "Vartiokylä Huivipolku", "Länsisatama 4"] },
              { name: "PM2.5", type: "number", values: [27.4, 12.1, 6.3, 4.8] },
              { name: "PM10", type: "number", values: [41.0, 19.6, 11.2, 9.9] },
            ],
          },
        ];
      default:
        return [
          {
            name: "BikeHireDockingStation",
            refId: "A",
            fields: [
              { name: "Station", type: "string", values: ["Kaivopuisto", "Laivasillankatu", "Kapteeninpuistikko", "Viiskulma", "Sepänkatu"] },
              { name: "Bikes", type: "number", values: [12, 0, 7, 3, 9] },
              { name: "Free", type: "number", values: [8, 20, 9, 13, 5] },
              { name: "Status", type: "string", values: ["working", "outOfService", "working", "working", "withIncidence"] },
            ],
          },
        ];
    }
  }
  const S1 = HOUR;
  switch (id) {
    case 1:
      return [series("", [1, 1, 1, 1], S1, "A", { instance: "antares-0" }), series("", [1, 1, 1, 0], S1, "A", { instance: "gateway-0" })];
    case 2:
      return [series("", wave(12, 0.62, 0.1, 1), S1, "A", { pod: "portal" })];
    case 3:
      return [
        {
          name: "",
          refId: "A",
          fields: [
            { name: "Time", type: "time", values: times(3, S1) },
            { name: "Value", type: "number", labels: { pod: "antares-0" }, values: [3.1e8, 3.2e8, 3.34e8] },
            { name: "Value", type: "number", labels: { pod: "portal" }, values: [7.0e8, 7.6e8, 8.12e8] },
            { name: "Value", type: "number", labels: { pod: "keycloak" }, values: [5.5e8, 5.6e8, 5.71e8] },
          ],
        },
      ];
    case 4:
    case 5:
      return [
        {
          name: "",
          refId: "A",
          fields: [
            { name: "route", type: "string", values: ["entities", "temporal", "health", "subscriptions"] },
            { name: "requests", type: "number", values: [5120, 830, 2400, 310] },
          ],
        },
      ];
    case 6:
      return ["0.05", "0.1", "0.5", "1", "+Inf"].map((le, i) => series("", wave(12, 40 - i * 7, 8, i).map((v) => Math.max(0, Math.round(v))), S1, "A", { le }));
    case 7:
      return [
        {
          name: "",
          refId: "A",
          fields: [
            { name: "Time", type: "time", values: [Date.now() - 60 * MIN, Date.now() - 40 * MIN, Date.now() - 25 * MIN, Date.now() - 10 * MIN] },
            { name: "air-quality", type: "string", values: ["ok", "failing", "ok", "running"] },
            { name: "bike-stations", type: "string", values: ["ok", "ok", "running", "ok"] },
          ],
        },
      ];
    default:
      return [series("pm25", wave(60, 9, 4, 3), S1)];
  }
}

const SHOWCASE = {
  uid: "panel-types",
  title: "Every panel type",
  time: { from: "now-1h", to: "now" },
  refresh: "",
  templating: {
    list: [{ name: "station", label: "Station", type: "custom", multi: false, includeAll: true, current: { text: "Kallio 2", value: "Kallio 2" }, options: [{ text: "Kallio 2", value: "Kallio 2" }, { text: "Mäkelänkatu", value: "Mäkelänkatu" }] }],
  },
  panels: [
    { id: 1, type: "stat", title: "Services up", gridPos: { x: 0, y: 0, w: 6, h: 6 }, fieldConfig: { defaults: { thresholds: { mode: "absolute", steps: [{ color: "red", value: null }, { color: "green", value: 1 }] }, mappings: [{ type: "value", options: { "1": { text: "Up" }, "0": { text: "Down" } } }] }, overrides: [] }, options: { colorMode: "background", reduceOptions: { calcs: ["lastNotNull"] } } },
    { id: 2, type: "gauge", title: "Portal CPU", gridPos: { x: 6, y: 0, w: 6, h: 6 }, fieldConfig: { defaults: { unit: "percentunit", min: 0, max: 1, thresholds: { mode: "absolute", steps: [{ color: "green", value: null }, { color: "orange", value: 0.7 }, { color: "red", value: 0.9 }] } }, overrides: [] }, options: { reduceOptions: { calcs: ["lastNotNull"] } } },
    { id: 3, type: "bargauge", title: "Resident memory by pod", gridPos: { x: 12, y: 0, w: 12, h: 6 }, fieldConfig: { defaults: { unit: "bytes", min: 0, max: 1e9, thresholds: { mode: "absolute", steps: [{ color: "green", value: null }, { color: "orange", value: 6e8 }, { color: "red", value: 8e8 }] } }, overrides: [] }, options: { reduceOptions: { calcs: ["lastNotNull"] } } },
    { id: 4, type: "barchart", title: "Requests by route", gridPos: { x: 0, y: 6, w: 12, h: 8 }, fieldConfig: { defaults: { unit: "short" }, overrides: [] }, options: {} },
    { id: 5, type: "piechart", title: "Share of requests", gridPos: { x: 12, y: 6, w: 12, h: 8 }, fieldConfig: { defaults: {}, overrides: [] }, options: { reduceOptions: { values: true } } },
    { id: 6, type: "heatmap", title: "Request duration buckets", gridPos: { x: 0, y: 14, w: 12, h: 8 }, fieldConfig: { defaults: {}, overrides: [] }, options: {} },
    { id: 7, type: "state-timeline", title: "Pipeline runs", gridPos: { x: 12, y: 14, w: 12, h: 8 }, fieldConfig: { defaults: { mappings: [{ type: "value", options: { ok: { text: "OK", color: "green" }, failing: { text: "Failing", color: "red" }, running: { text: "Running", color: "blue" } } }] }, overrides: [] }, options: {} },
    { id: 8, type: "histogram", title: "PM2.5 readings", gridPos: { x: 0, y: 22, w: 24, h: 7 }, fieldConfig: { defaults: {}, overrides: [] }, options: { bucketCount: 12 } },
  ],
};

const DASHBOARDS: Record<string, Record<string, unknown>> = {
  "jc-context-broker": deployed("context-broker"),
  "project-helsinki": deployed("helsinki"),
  "panel-types": SHOWCASE,
};

/** The panel as the Portal's route answers it: no targets, no datasource. */
function stripped(panel: Record<string, unknown>): Record<string, unknown> {
  const keep = ["id", "type", "title", "description", "gridPos", "fieldConfig", "options", "collapsed", "maxDataPoints"];
  const out = Object.fromEntries(Object.entries(panel).filter(([key]) => keep.includes(key)));
  if (Array.isArray(panel.panels)) out.panels = (panel.panels as Record<string, unknown>[]).map(stripped);
  return out;
}

function view(uid: string) {
  const board = DASHBOARDS[uid];
  const time = (board.time ?? {}) as { from?: string; to?: string };
  const list = ((board.templating as { list?: Record<string, unknown>[] } | undefined)?.list ?? []).map((v) => ({
    name: v.name,
    label: v.label ?? null,
    kind: v.type,
    multi: v.multi === true,
    includeAll: v.includeAll === true,
    options: ((v.options as { text: string; value: string }[] | undefined) ?? []).map((o) => ({ text: o.text, value: o.value })),
    current: (v.current as { value?: string } | undefined)?.value ?? null,
  }));
  return {
    uid,
    title: board.title,
    description: board.description ?? null,
    from: time.from ?? "now-6h",
    to: time.to ?? "now",
    refresh: board.refresh || null,
    variables: list,
    panels: (board.panels as Record<string, unknown>[]).map(stripped),
  };
}

type State = "filled" | "empty" | "error";

interface World {
  state: State;
  asked: string[];
}

async function stubApi(page: Page, world: World): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const panel = /\/grafana\/dashboards\/([^/]+)\/panels\/(\d+)$/.exec(url.pathname);
    if (panel) {
      world.asked.push(url.search);
      const [uid, id] = [decodeURIComponent(panel[1]), Number(panel[2])];
      if (world.state === "empty") return json({ frames: [{ name: "", refId: "A", fields: [{ name: "Time", type: "time", values: [] }, { name: "Value", type: "number", values: [] }] }] });
      if (world.state === "error") {
        return id === 4 && uid === "project-helsinki"
          ? json({ frames: [], accessDenied: "You cannot read the source of this panel: helsinki-bikes-ops" })
          : json({ frames: [], error: "the datasource did not answer: connection refused (prometheus:9090)" });
      }
      return json({ frames: filled(uid, id) });
    }
    const one = /\/grafana\/dashboards\/([^/]+)$/.exec(url.pathname);
    if (one) {
      const uid = decodeURIComponent(one[1]);
      return DASHBOARDS[uid] ? json(view(uid)) : json({ status: 404, title: "Not Found", detail: `no dashboard ${uid}` }, 404);
    }
    if (url.pathname.endsWith("/grafana/dashboards")) {
      return json({
        ready: true,
        items: [
          { uid: "jc-context-broker", title: "Context Broker — Antares", tags: ["antares", "platform"] },
          { uid: "project-helsinki", title: "Helsinki live", tags: ["helsinki"] },
          { uid: "panel-types", title: "Every panel type", tags: [] },
        ],
      });
    }
    if (url.pathname.endsWith("/grafana/options")) return json({ endpoints: [], types: [], attributes: [] });
    if (url.pathname.endsWith("/grafana")) return json({ ready: true, mayCreate: false, items: [] });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const SHOTS = process.env.DASHBOARD_SHOTS;
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

async function settle(page: Page): Promise<void> {
  await expect(page.getByRole("status", { name: /^Reading / })).toHaveCount(0);
  // Vega draws on the next frames after its module loads.
  await page.waitForTimeout(600);
}

/** The whole page at full height, so the sticky sidebar is not cut (KNOWLEDGE T-6423). */
async function pageShot(page: Page, name: string, width: number): Promise<void> {
  if (!SHOTS) return;
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.min(Math.max(height, 600), 6000) });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${SHOTS}/${name}-${width}.png` });
}

async function panelShot(panel: Locator, name: string, width: number): Promise<void> {
  if (!SHOTS) return;
  await panel.scrollIntoViewIfNeeded();
  await panel.screenshot({ path: `${SHOTS}/panel-${name}-${width}.png` });
}

const card = (page: Page, type: string) => page.locator(`section[data-panel-type="${type}"]`).first();

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("the Grafana page lists the project's dashboards and opens one", async ({ page }) => {
      await stubApi(page, { state: "filled", asked: [] });
      await page.goto("/projects/helsinki/grafana");
      const link = page.getByRole("link", { name: "Helsinki live" });
      await expect(link).toBeVisible();
      await pageShot(page, "list", width);
      await link.click();
      await expect(page).toHaveURL(/\/projects\/helsinki\/grafana\/dashboards\/project-helsinki$/);
      await expect(page.getByRole("heading", { level: 1, name: "Helsinki live" })).toBeVisible();
    });

    test("the deployment's context-broker dashboard draws its seven time series", async ({ page }) => {
      const world: World = { state: "filled", asked: [] };
      await stubApi(page, world);
      await page.goto("/projects/helsinki/grafana/dashboards/jc-context-broker");
      await expect(page.getByRole("heading", { level: 1, name: "Context Broker — Antares" })).toBeVisible();
      await settle(page);
      await expect(page.locator('section[data-panel-type="timeseries"]')).toHaveCount(7);
      await expect(page.getByRole("img", { name: /Transactions per second by status class: chart of 3 series/ })).toBeVisible();
      await expect(page.locator("canvas")).toHaveCount(7);
      expect(world.asked.every((search) => /from=now-6h&to=now&maxDataPoints=\d+$/.test(search))).toBe(true);
      await pageShot(page, "context-broker-filled", width);
      await panelShot(card(page, "timeseries"), "timeseries-filled", width);
      expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
    });

    test("the deployment's helsinki dashboard draws its legends, tables and mappings", async ({ page }) => {
      await stubApi(page, { state: "filled", asked: [] });
      await page.goto("/projects/helsinki/grafana/dashboards/project-helsinki");
      await settle(page);
      const cyclists = page.getByRole("region", { name: "Cyclists per day, by counting station" });
      await expect(cyclists.getByRole("table")).toBeVisible();
      await expect(cyclists.getByRole("columnheader", { name: "Last" })).toBeVisible();
      const air = page.getByRole("region", { name: "Air quality now, µg/m³" });
      await expect(air.getByText("27.4", { exact: true })).toBeVisible();
      const bikes = page.getByRole("region", { name: "City bikes, members" });
      await expect(bikes.getByText("Kaivopuisto")).toBeVisible();
      await pageShot(page, "helsinki-filled", width);
      await panelShot(card(page, "table"), "table-filled", width);
      expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
    });

    for (const state of ["filled", "empty", "error"] as const) {
      test(`every panel type, ${state}`, async ({ page }) => {
        const world: World = { state, asked: [] };
        await stubApi(page, world);
        await page.goto("/projects/helsinki/grafana/dashboards/panel-types");
        await expect(page.getByRole("heading", { level: 1, name: "Every panel type" })).toBeVisible();
        await settle(page);
        if (state === "filled") {
          await expect(page.getByRole("meter", { name: "portal" })).toHaveAttribute("aria-valuetext", "774 MiB");
          await expect(card(page, "stat").getByText("Down")).toBeVisible();
          expect(world.asked[0]).toContain("var-station=Kallio+2");
        } else if (state === "empty") {
          await expect(page.getByText("No data").first()).toBeVisible();
        } else {
          await expect(page.getByRole("alert").first()).toContainText("connection refused");
        }
        await pageShot(page, `types-${state}`, width);
        for (const type of ["stat", "gauge", "bargauge", "barchart", "piechart", "heatmap", "state-timeline", "histogram"]) {
          await panelShot(card(page, type), `${type}-${state}`, width);
        }
        expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
      });
    }

    test("the helsinki and context-broker dashboards, empty and failing", async ({ page }) => {
      for (const state of ["empty", "error"] as const) {
        await stubApi(page, { state, asked: [] });
        await page.goto("/projects/helsinki/grafana/dashboards/project-helsinki");
        await settle(page);
        if (state === "empty") await expect(page.getByText("No readings in this range").first()).toBeVisible();
        else await expect(page.getByText("You cannot read this panel's data")).toBeVisible();
        await pageShot(page, `helsinki-${state}`, width);
        await panelShot(card(page, "timeseries"), `timeseries-${state}`, width);
        await panelShot(card(page, "table"), `table-${state}`, width);
        await page.unrouteAll({ behavior: "ignoreErrors" });
      }
    });
  });
}

test("a time range, a variable and a refresh ask the panels again with them", async ({ page }) => {
  const world: World = { state: "filled", asked: [] };
  await stubApi(page, world);
  await page.goto("/projects/helsinki/grafana/dashboards/panel-types");
  await settle(page);
  world.asked.length = 0;
  await page.getByLabel("Time range", { exact: true }).selectOption("now-24h");
  await expect.poll(() => world.asked.some((search) => search.includes("from=now-24h"))).toBe(true);
  await page.getByLabel("Station").selectOption("$__all");
  await expect.poll(() => world.asked.some((search) => search.includes("var-station=%24__all"))).toBe(true);
  const before = world.asked.length;
  await page.getByRole("button", { name: "Refresh now" }).click();
  await expect.poll(() => world.asked.length).toBeGreaterThan(before);
});

test("a dashboard that is not there says so", async ({ page }) => {
  await stubApi(page, { state: "filled", asked: [] });
  await page.goto("/projects/helsinki/grafana/dashboards/gone");
  await expect(page.getByText("no dashboard gone")).toBeVisible();
  await pageShot(page, "missing", 1440);
});

test("the dashboard page has no axe violations", async ({ page }) => {
  await stubApi(page, { state: "filled", asked: [] });
  await page.goto("/projects/helsinki/grafana/dashboards/project-helsinki");
  await settle(page);
  expect(await axeViolations(page)).toEqual([]);
});
