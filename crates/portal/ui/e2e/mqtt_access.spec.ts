import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5876: a broker card's "who can do what" panel (the explain route, ADR-N-048 §2.13) in each
// state, and the ServiceAccount form offering the brokers' roles as grants. `vite preview` has no
// Portal API behind it, so the browser answers it; the server's own checks are the Rust tests of
// mqtt_access and mqtt_roles. SHOTS=<dir> writes 1440 and 400 shots of each state.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS;

const BROKER = {
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name: "air", namespace: PROJECT },
  spec: {
    roles: [
      { name: "sensor", publish: ["air/${clientid}/pm10"] },
      { name: "camera", serviceAccounts: ["old-cam"], publish: ["cams/${clientid}/#"], subscribe: ["cams/${clientid}/cmd"] },
    ],
  },
  status: { phase: "Live" },
};

const account = (name: string) => ({
  apiVersion: API,
  kind: "ServiceAccount",
  metadata: { name, namespace: PROJECT },
  spec: { owner: { user: "aino@hel.fi" }, purpose: "air sensors", credentials: [], roles: [] },
});

const ANSWERS: Record<string, unknown> = {
  "air-fleet": {
    broker: "air", account: "air-fleet", at: "2026-10-29T12:00:00Z",
    roles: [{ role: "sensor", publish: ["air/${clientid}/pm10"], subscribe: [], legacy: false, bound: true, until: "2026-12-01T00:00:00Z" }],
    outOfWindow: [],
  },
  "old-cam": {
    broker: "air", account: "old-cam", at: "2026-10-29T12:00:00Z",
    roles: [{ role: "camera", publish: ["cams/${clientid}/#"], subscribe: ["cams/${clientid}/cmd"], legacy: true, bound: false }],
    outOfWindow: [],
  },
  spare: { broker: "air", account: "spare", at: "2026-10-29T12:00:00Z", roles: [], outOfWindow: [] },
  lapsed: {
    broker: "air", account: "lapsed", at: "2026-10-29T12:00:00Z", roles: [],
    outOfWindow: [{ grant: "mqtt/*/sensor", notAfter: "2026-10-01T00:00:00Z" }],
  },
};

async function stubApi(page: Page, refused = false): Promise<void> {
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
      return json({ project: PROJECT, bootstrap: false, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount", "Role"], verbs: ["read", "propose"] } }] });
    }
    if (path.endsWith("/air/access")) {
      if (refused) return json({ status: 403, title: "Forbidden", detail: "you may not read the MQTT brokers of project 'helsinki'; ask a project admin for a role that reads MqttBroker" }, 403);
      return json(ANSWERS[url.searchParams.get("account") ?? ""]);
    }
    if (path.endsWith("/air/bindings")) return json({ broker: "air", bindings: [] });
    if (path.endsWith("/serviceaccounts")) return json({ apiVersion: API, kind: "List", items: Object.keys(ANSWERS).map(account) });
    if (path.endsWith("/mqttbrokers/air")) return json(BROKER);
    if (path.endsWith("/mqttbrokers")) return json({ apiVersion: API, kind: "List", items: [BROKER] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

async function shoot(page: Page, target: ReturnType<Page["locator"]>, state: string): Promise<void> {
  expect(await axeViolations(page), `axe on ${state}`).toEqual([]);
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await target.screenshot({ path: join(SHOTS, `access-${state}-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("who can do what on a broker", () => {
  test("each account's roles: bound, legacy, none and expired", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/mqttbrokers/air?lang=en`);
    const panel = page.getByRole("region", { name: "Who can do what" });
    const picker = panel.getByLabel("Service account");
    await expect(panel.getByRole("table", { name: "Roles of air that air-fleet holds now" })).toContainText("Role binding");
    await shoot(page, panel, "bound");
    await picker.selectOption("old-cam");
    await expect(panel.getByText(/the older way/)).toBeVisible();
    await shoot(page, panel, "legacy");
    await picker.selectOption("spare");
    await expect(panel.getByText("spare holds no role of this broker")).toBeVisible();
    await shoot(page, panel, "none");
    await picker.selectOption("lapsed");
    await expect(panel.getByText("Grants of this broker that are not in force")).toBeVisible();
    await shoot(page, panel, "expired");
  });

  test("a refusal says why", async ({ page }) => {
    await stubApi(page, true);
    await page.goto(`/projects/${PROJECT}/mqttbrokers/air?lang=en`);
    const panel = page.getByRole("region", { name: "Who can do what" });
    await expect(panel.getByText("You may not read this project's MQTT brokers")).toBeVisible();
    await shoot(page, panel, "refused");
  });

  test("the ServiceAccount form offers the brokers' roles as grants", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/settings/service-accounts?lang=en`);
    await page.getByRole("button", { name: "New service account" }).first().click();
    const dialog = page.getByRole("region", { name: "New service account" });
    const role = dialog.getByRole("combobox", { name: "Role", exact: true });
    await expect(role.locator("option", { hasText: "sensor on MQTT broker air" })).toHaveCount(1);
    await expect(role.locator("option", { hasText: "camera on every MQTT broker of the project" })).toHaveCount(1);
    await role.selectOption({ label: "sensor on MQTT broker air" });
    await shoot(page, dialog, "account-form");
  });
});
