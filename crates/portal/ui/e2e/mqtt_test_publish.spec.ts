/**
 * T-5534 (API/01 §44a): a steward opens "Test a message" on a broker card, dry-runs a device
 * message, reads each step, and watches the gateway's refusals. Shots at 1440 and 400 of each
 * state go to SHOTS_DIR when it is set.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;

const BROKER = {
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name: "cameras", namespace: PROJECT },
  spec: {
    roles: [
      { name: "cams", serviceAccounts: ["cam-west", "cam-east"], publish: ["cameras/${clientid}/#"] },
      { name: "ops", serviceAccounts: ["dashboard"], subscribe: ["dashboard/${clientid}/#"] },
    ],
  },
  status: { phase: "Live" },
};

const GREEN = {
  clientId: "helsinki-cam-west",
  published: false,
  steps: [
    { step: "acl", outcome: "ok", reason: "allowed by cameras/helsinki-cam-west/#" },
    { step: "size", outcome: "ok", reason: "22 of 65536 bytes" },
    { step: "rate", outcome: "ok", reason: "one message fits; a client may publish 10 per second (spec.limits.messagesPerSecond), the broker drops the rest" },
    { step: "binding", outcome: "ok", reason: "DataSource 'frames' reads it, Pipeline 'camera-frames' takes it" },
    { step: "mapping", outcome: "ok", reason: "1 record(s)" },
    { step: "endpoint", outcome: "ok", reason: "every attribute is written through helsinki-cameras" },
  ],
};

const REFUSED = {
  clientId: "helsinki-cam-west",
  published: false,
  steps: [
    { step: "acl", outcome: "refused", reason: "'cam-west' may publish only on cameras/helsinki-cam-west/#; the broker drops this message" },
    { step: "size", outcome: "refused", reason: "70000 bytes is above the broker's limit of 65536 (spec.limits.maxMessageBytes); the broker drops it" },
    { step: "rate", outcome: "ok", reason: "one message fits; a client may publish 10 per second (spec.limits.messagesPerSecond), the broker drops the rest" },
    { step: "binding", outcome: "refused", reason: "no DataSource of this project reads this topic from this broker: the message would arrive and nobody takes it; add the topic to a DataSource's mqtt.topics" },
    { step: "mapping", outcome: "unchecked", reason: "no Pipeline takes this topic" },
    { step: "endpoint", outcome: "unchecked", reason: "no Pipeline takes this topic" },
  ],
};

async function stubApi(page: Page, run: unknown): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname.endsWith("/test-publish")) return json(run);
    if (url.pathname.endsWith("/watch")) {
      return json({
        observedAt: new Date().toISOString(),
        refusals: [
          { at: new Date().toISOString(), reason: "expired", account: "cam-west" },
          { at: new Date(Date.now() - 4000).toISOString(), reason: "wrong_audience", account: "cam-west" },
        ],
        refusalsRead: true,
        connected: 0,
        messagesPerSecond: 0,
        reason: null,
      });
    }
    if (url.pathname.endsWith("/measured")) {
      return json({ measured: true, observedAt: new Date().toISOString(), messagesPerSecond: 0, publishers: 0, reason: null, accounts: [], rates: [], refusedPublishes: 0 });
    }
    if (url.pathname.endsWith("/mqttbrokers/cameras")) return json(BROKER);
    const plural = url.pathname.split("/").pop() ?? "";
    return json({ apiVersion: API, kind: "List", items: plural === "mqttbrokers" ? [BROKER] : [] });
  });
}

async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/${name}-${width}.png`, fullPage: false });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("Test a message on an MQTT broker", () => {
  for (const [state, run] of [
    ["green", GREEN],
    ["refused", REFUSED],
  ] as const) {
    test(`a steward dry-runs a device message: ${state}`, async ({ page }) => {
      await stubApi(page, run);
      await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
      await page.getByRole("button", { name: "Test a message" }).click();
      const dialog = page.getByRole("dialog", { name: "Test a message on cameras" });
      await expect(dialog.getByRole("combobox", { name: /Device account/ })).toHaveValue("cam-east");
      await dialog.getByRole("combobox", { name: /Device account/ }).selectOption("cam-west");
      await expect(dialog.getByRole("textbox", { name: /^Topic/ })).toHaveValue("cameras/helsinki-cam-west/reading");
      await shoot(page, "test-form");
      await dialog.getByRole("button", { name: "Run the test" }).click();
      await expect(dialog.locator("li[data-step]")).toHaveCount(6);
      await expect(dialog.locator('li[data-step="acl"]')).toHaveAttribute("data-outcome", state === "green" ? "ok" : "refused");
      expect(await axeViolations(page)).toEqual([]);
      await dialog.locator('li[data-step="endpoint"]').scrollIntoViewIfNeeded();
      await shoot(page, `test-${state}`);
    });
  }

  test("a steward watches for the device's real message and reads the refusal", async ({ page }) => {
    await stubApi(page, GREEN);
    await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
    await page.getByRole("button", { name: "Test a message" }).click();
    const dialog = page.getByRole("dialog", { name: "Test a message on cameras" });
    await dialog.getByRole("switch", { name: "Watch for the real message" }).click();
    await expect(dialog.getByText("The token has expired: fetch a new one.")).toBeVisible();
    await expect(dialog.getByText("The token was issued for another broker.")).toBeVisible();
    await dialog.getByText("The token has expired: fetch a new one.").scrollIntoViewIfNeeded();
    await shoot(page, "test-watch");
  });
});
