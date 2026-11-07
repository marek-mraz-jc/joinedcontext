import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-4860: the MQTT allowance end to end in the real build (PF-117…PF-119). An organization
// administrator turns MQTT on for a project with two brokers, a steward reads what is left and adds
// a broker, and the third one is refused with the limit named. `vite preview` has no Portal API
// behind it, so the browser answers it and keeps the allowance between the steps; the live run of
// the same journey is on dev after the deploy. Set MQTT_SHOTS to a directory to keep the screenshots.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.MQTT_SHOTS;

const meter = (limit: number, used: number) => ({ limit, used, left: Math.max(limit - used, 0) });

const broker = (name: string, spec: Record<string, unknown>) => ({
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name, namespace: PROJECT },
  spec: { roles: [{ name: "doors", serviceAccounts: ["door-1"], publish: ["doors/${clientid}/#"] }], ...spec },
  status: { phase: "Ready" },
});

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "admin", binding: "admins", rule: { kinds: ["RoleBinding", "Project", "MqttBroker"], verbs: ["read", "propose", "approve", "delete"] } }],
};
const STEWARD = {
  project: PROJECT,
  bootstrap: false,
  grants: [{ role: "steward", binding: "stewards", rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read", "propose"] } }],
};

interface World {
  admin: boolean;
  enabled: boolean;
  brokers: ReturnType<typeof broker>[];
  proposed: unknown[];
}

function allowance(world: World) {
  const used = world.brokers.length;
  return {
    enabled: world.enabled,
    origin: world.enabled ? "project" : "default",
    raise: `an org admin can raise this in Organization settings > Projects > ${PROJECT}`,
    modes: world.enabled ? ["ingest", "capture"] : [],
    addresses: world.enabled ? ["platform"] : [],
    ports: [],
    brokers: meter(world.enabled ? 2 : 0, used),
    connections: meter(world.enabled ? 1000 : 0, used * 100),
    messagesPerSecond: meter(world.enabled ? 10000 : 0, used * 1000),
    maxMessageBytes: { limit: world.enabled ? 65536 : 0 },
    portsInUse: [],
  };
}

async function stubApi(page: Page, world: World): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) return json(world.admin ? ADMIN : STEWARD);
    if (path.endsWith("/mqtt/allowance")) return json(allowance(world));
    if (path === `/api/v1/projects/org/projects/${PROJECT}` && request.method() === "GET") {
      return json({ apiVersion: API, kind: "Project", metadata: { name: PROJECT, namespace: "org" }, spec: { title: "Helsinki" } });
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft) {
      // The form saves its draft before it proposes (T-0769); the draft is not the change.
      if (request.method() !== "PUT") return route.fulfill({ status: 204 });
      const [, project, kind, name] = draft;
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({ project, kind, name, manifest, touchedBy: "steward", touchedKind: "person", version: 1, updatedAt: new Date().toISOString() });
    }
    if (request.method() === "PUT" || request.method() === "POST") {
      const body = request.postDataJSON() as { kind?: string };
      // The verdict refuses the broker past the allowance on the check and on the proposal alike.
      if (body.kind === "MqttBroker" && world.brokers.length >= 2) {
        return json(
          {
            type: "about:blank",
            title: "Forbidden",
            status: 403,
            detail: `MQTT allowance: brokers 2 reached (2 in use); an org admin can raise this in Organization settings > Projects > ${PROJECT}`,
          },
          403,
        );
      }
      if (url.searchParams.get("dryRun")) return json({ valid: true });
      world.proposed.push(body);
      return json({ apiVersion: API, kind: "Change", metadata: { name: `chg-${world.proposed.length}`, namespace: PROJECT }, spec: {}, status: { phase: "Proposed" } }, 201);
    }
    // One broker's own page (T-6103) reads it by name, with its panels' own reads.
    if (path.endsWith("/access")) return json({ roles: [], outOfWindow: [] });
    if (/\/mqttbrokers\/[^/]+\/bindings$/.test(path)) return json({ broker: "x", bindings: [] });
    if (path.endsWith("/watch")) return json({ observedAt: "2026-10-29T20:15:00Z", refusals: [], refusalsRead: false });
    const own = /\/mqttbrokers\/([^/]+)$/.exec(path);
    const named = own ? world.brokers.find((b) => (b as { metadata: { name: string } }).metadata.name === own[1]) : undefined;
    if (named) return json(named);
    const plural = path.split("/").pop() ?? "";
    if (plural === "projects") {
      return json({ apiVersion: API, kind: "List", items: [{ apiVersion: API, kind: "Project", metadata: { name: PROJECT, namespace: "org" }, spec: { title: "Helsinki" } }] });
    }
    if (plural === "serviceaccounts") {
      // The role's device is picked from the accounts the person reads (T-5590).
      return json({ apiVersion: API, kind: "List", items: [{ apiVersion: API, kind: "ServiceAccount", metadata: { name: "vent-1", namespace: PROJECT }, spec: {} }] });
    }
    return json({ apiVersion: API, kind: "List", items: plural === "mqttbrokers" ? world.brokers : [] });
  });
}

async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

/** Every New MQTT broker on the page is closed, with the allowance's words as its description. */
async function expectNewClosed(page: Page, reason: RegExp): Promise<void> {
  const buttons = page.getByRole("button", { name: "New MQTT broker" });
  await expect(buttons.first()).toHaveAttribute("aria-disabled", "true");
  for (const button of await buttons.all()) {
    await expect(button).toHaveAttribute("aria-disabled", "true");
    await expect(button).toHaveAccessibleDescription(reason);
  }
}

// The broker's address, its certificate and the device example show only under `portal.<domain>`.
const PORTAL_HOST = "portal.dev.joinedcontext.com";

test.describe("MQTT allowance", () => {
  test("admin enables MQTT, a steward adds a broker, the third is refused at the limit", async ({ page }) => {
    const world: World = { admin: false, enabled: false, brokers: [], proposed: [] };
    await stubApi(page, world);

    // Off: the steward reads who can turn it on.
    await page.goto(`/projects/${PROJECT}/mqttbrokers?lang=en`);
    await expect(page.getByText("MQTT is off for this project", { exact: true })).toBeVisible();
    await expect(page.getByText(/An organization administrator can turn it on in Administration › Projects › helsinki › MQTT\./).first()).toBeVisible();
    // No create action is offered while the allowance refuses it (T-5589).
    await expectNewClosed(page, /^MQTT is off for this project\. An organization administrator can turn it on/);
    await expect(page.getByText(/opens the form/)).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shoot(page, "mqtt-off");

    // The administrator opens the project's MQTT card and proposes the allowance.
    world.admin = true;
    await page.goto("/organization/projects?lang=en");
    const row = page.getByRole("row", { name: /^helsinki/ });
    await expect(row.getByText("Off", { exact: true })).toBeVisible();
    await row.getByRole("button", { name: "MQTT allowance of helsinki" }).click();
    // The edit form is a page of its own (`…/helsinki/edit`, UI-27).
    await page.getByRole("checkbox", { name: "MQTT brokers allowed" }).check();
    await page.getByRole("spinbutton", { name: "Brokers at most" }).fill("2");
    await page.getByRole("checkbox", { name: "Ingest and keep" }).check();
    expect(await axeViolations(page)).toEqual([]);
    await shoot(page, "mqtt-admin-enable");
    await page.getByRole("button", { name: "Propose change" }).click();
    await expect.poll(() => world.proposed.length).toBe(1);
    expect(world.proposed[0]).toMatchObject({ kind: "Project", spec: { title: "Helsinki", quotas: { mqtt: { enabled: true, brokers: 2, modes: ["capture"] } } } });

    // Approved by a second admin (T-4833): the allowance is in force.
    world.enabled = true;
    world.admin = false;
    world.brokers = [broker("doors", { mode: "capture", capture: { messagesPerTopic: 50, window: "PT2H" } })];
    // The build is served from 127.0.0.1; the browser asks for it under the Portal's own name.
    await page.route(`http://${PORTAL_HOST}:4173/**`, async (route) => {
      const url = route.request().url();
      if (new URL(url).pathname.startsWith("/api/")) return route.fallback();
      return route.fulfill({ response: await route.fetch({ url: url.replace(PORTAL_HOST, "127.0.0.1") }) });
    });
    await page.goto(`http://${PORTAL_HOST}:4173/projects/${PROJECT}/mqttbrokers/doors?lang=en`);
    await expect(page.getByText(/mosquitto_pub -h doors\.helsinki\.mqtt\.dev\.joinedcontext\.com -p 8883/)).toBeVisible();
    await expect(page.getByText(/-P "\$TOKEN"/)).toBeVisible();
    await expect(page.getByText("Keeps the last 50 messages per topic for 2 hours, for building the mapping.")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.goto(`http://${PORTAL_HOST}:4173/projects/${PROJECT}/mqttbrokers?lang=en`);
    await expect(page.getByText("1 of 2 brokers in use, 900 of 1,000 connections left, 9,000 messages per second left.")).toBeVisible();
    await shoot(page, "mqtt-capture");

    // The steward adds the second broker.
    await page.getByRole("button", { name: "New MQTT broker" }).first().click();
    await page
      .getByRole("combobox", { name: /What the broker does/ })
      .selectOption({ label: "…and keep the last messages so you can build the mapping" });
    // The window is a choice in words, never an ISO duration to type.
    await page.getByRole("combobox", { name: /^Kept for/ }).selectOption({ label: "2 hours" });
    await shoot(page, "mqtt-add-broker");
    await page.goBack();

    // At the limit the page says so, and the API's refusal names it.
    world.brokers = [...world.brokers, broker("gates", {})];
    await page.goto(`/projects/${PROJECT}/mqttbrokers?lang=en`);
    await expect(page.locator("[data-allowance='full']")).toContainText(/All 2 brokers are in use: a new one is refused\./);
    await expectNewClosed(page, /^All 2 brokers are in use: a new one is refused\./);
    await shoot(page, "mqtt-full");
    // The form's own address still opens it, and the API's refusal stays the gate.
    await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
    await page.getByRole("textbox", { name: /^Name/ }).first().fill("vents");
    await page.getByRole("textbox", { name: /^Role/ }).first().fill("vents");
    await page.getByRole("combobox", { name: /Service accounts/ }).first().selectOption("vent-1");
    await page.getByRole("button", { name: "Check", exact: true }).click();
    await expect(page.getByText(/MQTT allowance: brokers 2 reached/)).toBeVisible();
    // The verdict's reason is said once, not again beside the submit (T-5589).
    await expect(page.locator("span:not(.sr-only)", { hasText: /^Run the check first/ })).toHaveCount(1);
    await shoot(page, "mqtt-refused");
  });
});
