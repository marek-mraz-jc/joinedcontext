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
              { name: "Status", type: "string", values: ["working", "outOfService", "working", "working", "outOfService"] },
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
    { id: 6, type: "heatmap", title: "Request duration buckets", gridPos: { x: 0, y: 14, w: 12, h: 8 }, fieldConfig: { defaults: {}, overrides: [] }, options: { yAxis: { unit: "s" } } },
    { id: 7, type: "state-timeline", title: "Pipeline runs", gridPos: { x: 12, y: 14, w: 12, h: 8 }, fieldConfig: { defaults: { mappings: [{ type: "value", options: { ok: { text: "OK", color: "green" }, failing: { text: "Failing", color: "red" }, running: { text: "Running", color: "blue" } } }] }, overrides: [] }, options: {} },
    { id: 8, type: "histogram", title: "PM2.5 readings", gridPos: { x: 0, y: 22, w: 24, h: 7 }, fieldConfig: { defaults: {}, overrides: [] }, options: { bucketCount: 12 } },
  ],
};

// Dev's one geomap (org 3, "Air quality map"), its frame daiya's own answer for the 100 helsinki
// AirQualityObserved of the dev export (crates/daiya/query/tests/ngsild.rs writes it).
const GEOMAP = "pv-97058e3a30c95eb36b80743122d56561";
const geomapFrames = JSON.parse(readFileSync(join(here, "../tests/fixtures/geomap_frame.json"), "utf8")) as Frame[];
/** The geomap frame with the location of the first `n` rows gone. */
const unlocated = (n: number): Frame[] =>
  geomapFrames.map((frame) => ({ ...frame, fields: frame.fields.map((field) => (field.name === "location" ? { ...field, values: field.values.map((value, row) => (row < n ? null : value)) } : field)) }));

const DASHBOARDS: Record<string, Record<string, unknown>> = {
  [GEOMAP]: deployed("air-quality-map"),
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
    current: [(v.current as { value?: string } | undefined)?.value].filter((value) => value !== undefined),
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

type State = "filled" | "empty" | "error" | "unlocated" | "nolocation";

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
      if (uid === GEOMAP && world.state !== "error") {
        return json({ frames: world.state === "empty" ? geomapFrames.map((frame) => ({ ...frame, fields: frame.fields.map((field) => ({ ...field, values: [] })) })) : world.state === "unlocated" ? unlocated(12) : world.state === "nolocation" ? unlocated(100) : geomapFrames });
      }
      if (world.state === "empty") return json({ frames: [{ name: "", refId: "A", fields: [{ name: "Time", type: "time", values: [] }, { name: "Value", type: "number", values: [] }] }] });
      if (world.state === "error") {
        // helsinki: one source refuses the person, one fails, the others answer; every other
        // dashboard: every source fails, which the page sums up above the panels.
        if (uid === "project-helsinki" && id === 4) return json({ frames: [], accessDenied: "You cannot read the source of this panel: helsinki-bikes-ops" });
        if (uid === "project-helsinki" && id !== 1) return json({ frames: filled(uid, id) });
        return json({ frames: [], error: "the datasource did not answer: connection refused (prometheus:9090)" });
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
    // This installation configures no basemap, as jc's deployment does not: the route answers 404.
    if (url.pathname.includes("/basemap/")) return json({ status: 404, title: "Resource Not Found", detail: "basemap is not configured on this platform" }, 404);
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
  // The page first: before it renders there is no "Reading" status to wait out either.
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
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

/**
 * Taps the densest marker spots of the map until one tooltip lists several stacked rows (the
 * helsinki vehicles report one place several times), and shoots it.
 */
async function stackedPopup(page: Page, map: Locator, width: number): Promise<void> {
  const canvas = map.locator("canvas");
  const png = (await canvas.screenshot()).toString("base64");
  const spots = await page.evaluate(async (data) => {
    const image = new Image();
    image.src = `data:image/png;base64,${data}`;
    await image.decode();
    const context = Object.assign(document.createElement("canvas"), { width: image.width, height: image.height }).getContext("2d")!;
    context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, image.width, image.height).data;
    const orange = (x: number, y: number) => {
      const i = (y * image.width + x) * 4;
      return pixels[i] > 200 && pixels[i + 1] > 110 && pixels[i + 1] < 190 && pixels[i + 2] < 120;
    };
    const found: { x: number; y: number; score: number }[] = [];
    for (let y = 12; y < image.height - 12; y += 3)
      for (let x = 12; x < image.width - 12; x += 3) {
        if (!orange(x, y)) continue;
        let score = 0;
        for (let dy = -12; dy <= 12; dy += 3) for (let dx = -12; dx <= 12; dx += 3) if (orange(x + dx, y + dy)) score++;
        found.push({ x, y, score });
      }
    // Every marker's centre once: the most orange spot of each 10 px neighbourhood.
    found.sort((a, b) => b.score - a.score);
    const kept: { x: number; y: number }[] = [];
    for (const spot of found) if (kept.every((other) => Math.hypot(other.x - spot.x, other.y - spot.y) > 10)) kept.push(spot);
    return { kept, ratio: image.width };
  }, png);
  const box = (await canvas.boundingBox())!;
  const scale = spots.ratio / box.width;
  const popup = page.locator(".maplibregl-popup");
  for (const spot of spots.kept) {
    await page.mouse.click(box.x + spot.x / scale, box.y + spot.y / scale);
    if ((await popup.locator("dl").count()) > 1) break;
  }
  await expect(popup.locator("dl").nth(1)).toBeVisible();
  await expect(popup.getByText(/Volvo \(pesuauto\)|Wille \(harjakone\)/).first()).toBeVisible();
  await pageShot(page, "geomap-popup", width);
  // A tap on the bare map closes the tooltip.
  await page.mouse.click(box.x + 4, box.y + box.height - 4);
  await expect(popup).toHaveCount(0);
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
      // Each panel asks the dashboard's six hours as epochs, as Grafana asks its datasources.
      expect(world.asked.every((search) => { const query = new URLSearchParams(search); return Number(query.get("to")) - Number(query.get("from")) === 6 * HOUR && [...query.keys()].join() === "from,to,maxDataPoints"; })).toBe(true);
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
          await expect(page.getByRole("alert").first()).toContainText("8 panels could not be read");
          await expect(card(page, "gauge").getByText("Not read: see the notice above the panels.")).toBeVisible();
        }
        await pageShot(page, `types-${state}`, width);
        for (const type of ["stat", "gauge", "bargauge", "barchart", "piechart", "heatmap", "state-timeline", "histogram"]) {
          await panelShot(card(page, type), `${type}-${state}`, width);
        }
        expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
      });
    }

    test("dev's air quality geomap draws its markers, empty, with some rows and with no row located", async ({ page }) => {
      for (const state of ["filled", "empty", "unlocated", "nolocation"] as const) {
        await stubApi(page, { state, asked: [] });
        await page.goto(`/projects/helsinki/grafana/dashboards/${GEOMAP}`);
        await expect(page.getByRole("heading", { level: 1, name: "Air quality map (t6516-mw3xzrhm)" })).toBeVisible();
        await settle(page);
        const map = card(page, "geomap");
        if (state === "empty") {
          await expect(map.getByText("No data")).toBeVisible();
          await expect(map.getByRole("button", { name: "Show a longer range" })).toHaveCount(0);
        } else if (state === "nolocation") {
          await expect(map.getByText("No row has a location")).toBeVisible();
          await expect(map.getByText("None of the 100 rows has a location: a GeoJSON point (location, geometry) or latitude and longitude fields. The tables show the rows.")).toBeVisible();
          await expect(map.getByRole("link", { name: "Open as tables" })).toHaveAttribute("href", /^\/dashboards\/d\/pv-97058e3a30c95eb36b80743122d56561\?/);
          await expect(map.locator("canvas")).toHaveCount(0);
        } else {
          await expect(map.getByRole("application", { name: `Map of Air quality map (t6516-mw3xzrhm): ${state === "filled" ? 100 : 88} markers` })).toBeVisible();
          await expect(map.locator("canvas")).toHaveCount(1);
          await expect(map.getByText(`Markers: ${state === "filled" ? 100 : 88}, latest values; the time range does not narrow them · no basemap, positions only`)).toBeVisible();
          await expect(map.getByText(/shown as a table/)).toHaveCount(0);
          if (state === "unlocated") await expect(map.getByText("12 of 100 rows have no location and are not drawn.")).toBeVisible();
          // The markers are drawn: a shot of the map holds the layer's orange (#FF9830).
          await expect.poll(async () => {
            const png = (await map.locator("canvas").screenshot()).toString("base64");
            return page.evaluate(async (data) => {
              const image = new Image();
              image.src = `data:image/png;base64,${data}`;
              await image.decode();
              const context = Object.assign(document.createElement("canvas"), { width: image.width, height: image.height }).getContext("2d")!;
              context.drawImage(image, 0, 0);
              const pixels = context.getImageData(0, 0, image.width, image.height).data;
              let orange = 0;
              for (let i = 0; i < pixels.length; i += 4) if (pixels[i] > 200 && pixels[i + 1] > 110 && pixels[i + 1] < 190 && pixels[i + 2] < 120) orange++;
              return orange;
            }, png);
          }, { timeout: 10_000 }).toBeGreaterThan(100);
        }
        await pageShot(page, `geomap-${state}`, width);
        expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
        if (state === "filled") await stackedPopup(page, map, width);
        await page.unrouteAll({ behavior: "ignoreErrors" });
      }
    });

    test("the helsinki and context-broker dashboards, empty and failing", async ({ page }) => {
      for (const state of ["empty", "error"] as const) {
        await stubApi(page, { state, asked: [] });
        await page.goto("/projects/helsinki/grafana/dashboards/project-helsinki");
        await settle(page);
        if (state === "empty") {
          await expect(page.getByText("No readings in this range").first()).toBeVisible();
          await expect(card(page, "timeseries").getByText("No readings in “Last 7 days”. Show a longer range.")).toBeVisible();
        } else {
          await expect(page.getByText("You cannot read this panel's data")).toBeVisible();
          await expect(card(page, "timeseries").getByRole("alert")).toContainText("The data source did not answer");
        }
        await pageShot(page, `helsinki-${state}`, width);
        await panelShot(card(page, "timeseries"), `timeseries-${state}`, width);
        await panelShot(state === "empty" ? card(page, "table") : page.locator('section[data-panel-type="table"]').last(), `table-${state}`, width);
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
  await page.getByText("Last 1 hour", { exact: true }).first().click();
  await page.getByRole("button", { name: "Last 24 hours" }).click();
  await expect.poll(() => world.asked.some((search) => { const query = new URLSearchParams(search); return Number(query.get("to")) - Number(query.get("from")) === 24 * HOUR; })).toBe(true);
  await page.getByLabel("Station").selectOption("$__all");
  await expect.poll(() => world.asked.some((search) => search.includes("var-station=%24__all"))).toBe(true);
  const before = world.asked.length;
  await page.getByRole("button", { name: "Refresh now" }).click();
  await expect.poll(() => world.asked.length).toBeGreaterThan(before);
});

test("a dashboard that is not there says so", async ({ page }) => {
  await stubApi(page, { state: "filled", asked: [] });
  await page.goto("/projects/helsinki/grafana/dashboards/gone");
  await expect(page.getByText("This dashboard is not in the project")).toBeVisible();
  await expect(page.getByRole("link", { name: "Back to the dashboards" })).toBeVisible();
  await pageShot(page, "missing", 1440);
  await pageShot(page, "missing", 400);
});

test("the dashboard page has no axe violations", async ({ page }) => {
  await stubApi(page, { state: "filled", asked: [] });
  await page.goto("/projects/helsinki/grafana/dashboards/project-helsinki");
  await settle(page);
  expect(await axeViolations(page)).toEqual([]);
  await page.goto(`/projects/helsinki/grafana/dashboards/${GEOMAP}`);
  await settle(page);
  await expect(page.locator('section[data-panel-type="geomap"] canvas')).toHaveCount(1);
  expect(await axeViolations(page)).toEqual([]);
});
