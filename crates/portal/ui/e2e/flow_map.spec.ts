import { expect, test } from "@playwright/test";
import type { Page, TestInfo } from "@playwright/test";
import { axeViolations } from "./axe";

// T-4862: the data flow map of helsinki, from the HSL MQTT feed and a platform broker through the
// pipelines into the spaces, and on through a derived and two KPI pipelines. `vite preview` has
// no Portal API behind it, so the browser answers it with the map the API would give; the
// layout, the clicks and the editor opened on the chosen source are the real ones.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";

type Kind = "MqttBroker" | "DataSource" | "Pipeline" | "Endpoint" | "ContextSpace";
const id = (kind: Kind, name: string, project = PROJECT) => `${project}/${kind}/${name}`;
const node = (kind: Kind, name: string, over: Record<string, unknown> = {}) => ({
  id: id(kind, name),
  kind,
  name,
  project: PROJECT,
  opaque: false,
  ...over,
});
const run = (sent: number, rejected = 0, failed = 0) => ({
  at: "2026-10-21T04:00:00Z",
  sent,
  rejected,
  failed,
  perMinute: Math.round(sent / 6) / 10,
});
const granted = (endpoint: string, operation: string, policy: string) => ({ state: "granted", endpoint, operation, policy });
const edge = (from: string, to: string, relation: string, grant?: unknown) => ({ from, to, relation, ...(grant ? { grant } : {}) });

const MAP = {
  project: PROJECT,
  nodes: [
    node("MqttBroker", "sensors", { title: "City sensors broker" }),
    node("DataSource", "hsl-hfp", { title: "HSL vehicle positions", sourceType: "mqtt", hosts: ["mqtt.hsl.fi:8883"], topics: ["/hfp/v2/journey/ongoing/vp/#"] }),
    node("DataSource", "air-sensors", { sourceType: "mqtt", topics: ["air/+/reading"] }),
    node("Pipeline", "hsl-vehicles", { title: "HSL vehicles", role: "ingest", phase: "Live", enabled: true, lastRun: run(18240, 12) }),
    node("Pipeline", "air-ingest", { role: "ingest", phase: "Live", enabled: true, lastRun: run(960) }),
    node("Pipeline", "road-works", { role: "derived", phase: "Live", enabled: true, lastRun: run(42) }),
    node("Endpoint", "transport-write", { space: "transport", slug: "transport" }),
    node("Endpoint", "air-write", { space: "air", slug: "air" }),
    { id: id("Endpoint", "roads-shared", "espoo"), kind: "Endpoint", name: "roads-shared", project: "espoo", opaque: true },
    node("ContextSpace", "transport", { title: "Transport" }),
    node("ContextSpace", "air", { title: "Air quality" }),
    node("Endpoint", "transport-read", { space: "transport", slug: "transport-read" }),
    node("Endpoint", "air-read", { space: "air", slug: "air-read" }),
    node("Pipeline", "fleet-kpi", { title: "Fleet on time", role: "kpi", phase: "Live", enabled: true, lastRun: run(1) }),
    node("Pipeline", "air-kpi", { role: "kpi", phase: "Live", enabled: false }),
    node("Pipeline", "stop-delays", { title: "Delays per stop", role: "derived", phase: "Error", enabled: true, lastRun: run(310, 0, 310) }),
    node("Endpoint", "kpi-write", { space: "helsinki-kpi", slug: "kpi" }),
    node("Endpoint", "stops-write", { space: "stops", slug: "stops" }),
    node("ContextSpace", "helsinki-kpi", { title: "Indicators", role: "kpi" }),
    node("ContextSpace", "stops", { title: "Stops" }),
  ],
  edges: [
    edge(id("MqttBroker", "sensors"), id("DataSource", "air-sensors"), "connects"),
    edge(id("DataSource", "hsl-hfp"), id("Pipeline", "hsl-vehicles"), "feeds"),
    edge(id("DataSource", "air-sensors"), id("Pipeline", "air-ingest"), "feeds"),
    edge(id("Pipeline", "hsl-vehicles"), id("Endpoint", "transport-write"), "writes", granted("transport-write", "upsertBatch", "transport-pipelines")),
    edge(id("Pipeline", "air-ingest"), id("Endpoint", "air-write"), "writes", granted("air-write", "upsertBatch", "air-pipelines")),
    edge(id("Endpoint", "roads-shared", "espoo"), id("Pipeline", "road-works"), "reads", { state: "external", endpoint: "roads-shared", operation: "queryEntity" }),
    edge(id("Pipeline", "road-works"), id("Endpoint", "transport-write"), "writes", granted("transport-write", "upsertBatch", "transport-pipelines")),
    edge(id("Endpoint", "transport-write"), id("ContextSpace", "transport"), "stores"),
    edge(id("Endpoint", "air-write"), id("ContextSpace", "air"), "stores"),
    edge(id("ContextSpace", "transport"), id("Endpoint", "transport-read"), "exposes"),
    edge(id("ContextSpace", "air"), id("Endpoint", "air-read"), "exposes"),
    edge(id("Endpoint", "transport-read"), id("Pipeline", "fleet-kpi"), "reads", granted("transport-read", "queryEntity", "transport-pipelines")),
    edge(id("Endpoint", "transport-read"), id("Pipeline", "stop-delays"), "reads", granted("transport-read", "queryEntity", "transport-pipelines")),
    edge(id("Endpoint", "air-read"), id("Pipeline", "air-kpi"), "reads", granted("air-read", "queryEntity", "air-pipelines")),
    edge(id("Pipeline", "fleet-kpi"), id("Endpoint", "kpi-write"), "writes", granted("kpi-write", "upsertBatch", "kpi-pipelines")),
    edge(id("Pipeline", "air-kpi"), id("Endpoint", "kpi-write"), "writes", granted("kpi-write", "upsertBatch", "kpi-pipelines")),
    edge(id("Pipeline", "stop-delays"), id("Endpoint", "stops-write"), "writes", {
      state: "missing",
      endpoint: "stops-write",
      operation: "upsertBatch",
      fix: "add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch",
    }),
    edge(id("Endpoint", "kpi-write"), id("ContextSpace", "helsinki-kpi"), "stores"),
    edge(id("Endpoint", "stops-write"), id("ContextSpace", "stops"), "stores"),
  ],
};

const datasource = (name: string) => ({
  apiVersion: API,
  kind: "DataSource",
  metadata: { name, namespace: PROJECT },
  spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.hsl.fi:8883"], topics: ["/hfp/v2/journey/ongoing/vp/#"] } },
});

async function stubApi(page: Page, map: unknown = MAP): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname.endsWith("/flow-map")) return json(map);
    const plural = url.pathname.split("/").pop() ?? "";
    const items = plural === "datasources" ? [datasource("hsl-hfp"), datasource("air-sensors")] : [];
    return json({ apiVersion: API, kind: "List", items });
  });
}

/** Where the screenshots go: the task's evidence folder when SHOTS_DIR names one. */
function shot(info: TestInfo, name: string): string {
  return process.env.SHOTS_DIR ? `${process.env.SHOTS_DIR}/${name}.png` : info.outputPath(`${name}.png`);
}

test.describe("data flow map", () => {
  test("a steward follows the HSL path, finds the missing grant and sends a source to a space", async ({ page }, info) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
    await page.getByRole("tab", { name: "Data flow map" }).click();
    await expect(page).toHaveURL(/view=map/);

    const map = page.getByRole("group", { name: "Data flow map" });
    await expect(map.getByRole("button", { name: /^Pipeline: HSL vehicles\./ })).toBeVisible();
    await expect(page.getByText("1 path lacks the grant it needs")).toBeVisible();
    await expect(page.getByRole("status").filter({ hasText: "add a Policy on space stops" })).toBeVisible();
    // Left to right: source, pipeline, endpoint, space, then the derived pipeline further right.
    const x = async (name: RegExp) => (await map.getByRole("button", { name }).boundingBox())?.x ?? -1;
    expect(await x(/^Data source: HSL vehicle positions/)).toBeLessThan(await x(/^Pipeline: HSL vehicles/));
    expect(await x(/^Space: Transport/)).toBeLessThan(await x(/^Pipeline: Delays per stop/));
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: shot(info, "flow-map-1440"), fullPage: true });

    await map.getByRole("button", { name: /^Pipeline: Delays per stop/ }).click();
    const detail = page.getByRole("heading", { level: 2, name: "Delays per stop" });
    await expect(detail).toBeFocused();
    await expect(page.getByText("Fix: add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch")).toBeVisible();
    await expect(page.getByRole("button", { name: "Runs" })).toBeVisible();
    await page.screenshot({ path: shot(info, "flow-map-detail-1440"), fullPage: true });

    await map.getByRole("button", { name: /^Endpoint: roads-shared/ }).click();
    await expect(page.getByText("Project espoo; you cannot read its details")).toBeVisible();

    await map.getByRole("button", { name: /^Data source: HSL vehicle positions/ }).click();
    await page.getByLabel("Write into").selectOption({ label: "transport-write (space transport)" });
    await page.screenshot({ path: shot(info, "flow-map-send-1440"), fullPage: true });
    await page.getByRole("button", { name: "Send to a space" }).click();
    await expect(page).toHaveURL(/\/pipelines\/new/);
    await expect(page.getByTestId("form-page")).toBeVisible();
    await page.screenshot({ path: shot(info, "flow-map-editor-1440"), fullPage: true });
  });

  test("a phone reads the same map as a grouped list", async ({ page }, info) => {
    await page.setViewportSize({ width: 400, height: 900 });
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/pipelines?view=map&lang=en`);
    await expect(page.getByRole("heading", { level: 2, name: "Sources" })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: "Spaces" })).toBeVisible();
    await expect(page.getByRole("group", { name: "Data flow map" })).toBeHidden();
    const width = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(width).toBeLessThanOrEqual(400);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: shot(info, "flow-map-400"), fullPage: true });
    await page.getByRole("button", { name: /^Pipeline: Delays per stop/ }).click();
    await expect(page.getByRole("heading", { level: 2, name: "Delays per stop" })).toBeFocused();
    await page.screenshot({ path: shot(info, "flow-map-detail-400"), fullPage: true });
  });

  test("an empty project says how a path starts", async ({ page }, info) => {
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 800 });
      await stubApi(page, { project: PROJECT, nodes: [], edges: [] });
      await page.goto(`/projects/${PROJECT}/pipelines?view=map&lang=en`);
      await expect(page.getByText("Nothing flows in this project yet.")).toBeVisible();
      await page.screenshot({ path: shot(info, `flow-map-empty-${width}`), fullPage: true });
    }
  });

  test("a map the API refuses says why", async ({ page }, info) => {
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 800 });
      await stubApi(page);
      await page.route("**/flow-map*", (route) =>
        route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "The mirror is not loaded yet." }),
        }),
      );
      await page.goto(`/projects/${PROJECT}/pipelines?view=map&lang=en`);
      await expect(page.getByRole("alert")).toContainText("The mirror is not loaded yet.");
      await page.screenshot({ path: shot(info, `flow-map-error-${width}`), fullPage: true });
    }
  });
});
