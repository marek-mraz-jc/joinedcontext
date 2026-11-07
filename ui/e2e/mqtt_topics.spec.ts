import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5533: a broker card's topics (which topic fills which attribute) and a capture broker's kept
// messages, for a steward who may approve brokers and for an editor who may not. `vite preview`
// has no Portal API behind it, so the browser answers it; the server's own checks are the Rust
// tests of mqtt_bindings and mqtt_captured. SHOTS=<dir> writes 1440 and 400 shots of each state.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS;

const BROKER = {
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name: "sensors", namespace: PROJECT },
  spec: {
    mode: "capture",
    capture: { messagesPerTopic: 20, window: "PT1H" },
    roles: [{ name: "devices", serviceAccounts: ["s-1"], publish: ["devices/${clientid}/data"] }],
  },
  status: { phase: "Live" },
};

const BOUND = [
  {
    topicFilter: "devices/+/data",
    dataSource: { kind: "DataSource", name: "sensors-in" },
    pipeline: { kind: "Pipeline", name: "weather" },
    endpoint: { kind: "Endpoint", name: "weather-write", space: "weather" },
    type: "WeatherObserved",
    attributes: [
      { from: "temp", to: "temperature" },
      { from: "hum", to: "relativeHumidity" },
      { from: "lon, lat", to: "location" },
      { to: "dateObserved" },
    ],
  },
  { topicFilter: "devices/+/status", dataSource: { kind: "DataSource", name: "status-in" } },
];

const HIDDEN = [{ dataSource: { kind: "DataSource", hidden: true } }];

const MESSAGES = [
  {
    topic: "devices/s-1/data",
    receivedAt: "2026-10-24T20:41:02Z",
    clientId: "helsinki-s-1",
    bytes: 52,
    payload: '{"temp":21.5,"hum":40,"lon":24.94,"lat":60.17}',
    encoding: "utf-8",
  },
  { topic: "devices/s-2/data", receivedAt: "2026-10-24T20:40:11Z", bytes: 3, payload: "/wAB", encoding: "base64" },
];

interface World {
  approver: boolean;
  bindings: unknown[];
  messages: unknown[];
}

async function stubApi(page: Page, world: World): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = world.approver ? ["read", "propose", "approve"] : ["read", "propose"];
      return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["MqttBroker", "DataSource", "Pipeline", "Endpoint"], verbs } }] });
    }
    if (path.endsWith("/sensors/bindings")) return json({ broker: "sensors", bindings: world.bindings });
    if (path.endsWith("/sensors/captured")) {
      // The server's own rule (PL-87): anybody but an approver gets the 404 of no broker.
      if (!world.approver) return json({ status: 404, detail: "MQTT broker 'sensors' not found" }, 404);
      return json({ broker: "sensors", mode: "capture", window: "PT1H", messagesPerTopic: 20, messages: world.messages, truncated: false });
    }
    if (path.endsWith("/mqttbrokers/sensors")) return json(BROKER);
    if (path.endsWith("/mqttbrokers")) return json({ apiVersion: API, kind: "List", items: [BROKER] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

async function shoot(page: Page, state: string): Promise<void> {
  expect(await axeViolations(page), `axe on ${state}`).toEqual([]);
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.locator('section[aria-labelledby="broker-sensors"]').screenshot({ path: join(SHOTS, `topics-${state}-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function open(page: Page, world: World): Promise<string[]> {
  const asked: string[] = [];
  page.on("request", (request) => asked.push(`${request.method()} ${new URL(request.url()).pathname}`));
  await stubApi(page, world);
  await page.goto(`/projects/${PROJECT}/mqttbrokers/sensors?lang=en`);
  await expect(page.getByRole("heading", { level: 1, name: "sensors" })).toBeVisible();
  return asked;
}

test.describe("a broker's topics and captured messages", () => {
  test("a steward sees which topic fills which attribute and the newest kept messages, and clears them", async ({ page }) => {
    await open(page, { approver: true, bindings: BOUND, messages: MESSAGES });
    const topics = page.getByRole("table", { name: "Which topic of sensors fills which attribute" });
    const row = topics.getByRole("row", { name: /devices\/\+\/data/ });
    await expect(row).toContainText("WeatherObserved");
    await expect(row).toContainText("temp → temperature");
    await expect(row.getByRole("link", { name: "weather-write" })).toHaveAttribute("href", `/projects/${PROJECT}/endpoints/weather-write`);
    await expect(topics.getByRole("row", { name: /devices\/\+\/status/ })).toContainText("No pipeline reads it yet");
    await expect(page.getByLabel("Payload of devices/s-1/data")).toContainText('"temp": 21.5');
    await shoot(page, "bindings-capture");

    await page.getByRole("button", { name: "Delete captured messages" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("Delete every captured message of sensors?");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
  });

  test("a hop the steward may not read is hidden, and an empty capture says what comes next", async ({ page }) => {
    await open(page, { approver: true, bindings: HIDDEN, messages: [] });
    await expect(page.getByText("A DataSource you may not read")).toBeVisible();
    await expect(page.getByText("No message kept yet")).toBeVisible();
    await shoot(page, "hidden-capture-empty");
  });

  test("a broker with no bound topic tells the steward how to bind one", async ({ page }) => {
    await open(page, { approver: true, bindings: [], messages: MESSAGES });
    await expect(page.getByText("No topic is bound yet")).toBeVisible();
    await shoot(page, "no-binding");
  });

  test("an editor who may not approve brokers sees the topics, no capture, and never asks for it", async ({ page }) => {
    const asked = await open(page, { approver: false, bindings: BOUND, messages: MESSAGES });
    await expect(page.getByRole("table", { name: "Which topic of sensors fills which attribute" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Captured messages" })).toHaveCount(0);
    expect(asked.filter((request) => request.endsWith("/captured"))).toEqual([]);
    await shoot(page, "refused");
  });
});
