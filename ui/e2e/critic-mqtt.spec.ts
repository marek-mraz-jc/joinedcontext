import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-5524, UI critic 4/6: the MQTT pages walked as steward, editor and viewer, every state shot at
// 1440 and 400, axe per state, and the time from navigation to the first heading. Dev runs no MQTT
// broker until T-4857, so the browser answers the API here (the same stubs as mqtt_allowance.spec.ts)
// and the timings are of the build, not of a live round trip. The walk records; it asserts only
// what the page must show for the state to be the one it claims, and that axe finds nothing.
// Set CRITIC_SHOTS to a directory to keep the shots and `critic-mqtt.jsonl` (state, persona, ms).

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.CRITIC_SHOTS;

type Persona = "steward" | "editor" | "viewer";

const VERBS: Record<Persona, string[]> = {
  steward: ["read", "propose", "approve"],
  editor: ["read", "propose"],
  viewer: ["read"],
};

const meter = (limit: number, used: number) => ({
  limit,
  used,
  left: Math.max(limit - used, 0),
});

const broker = (name: string, phase: string) => ({
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name, namespace: PROJECT },
  spec: {
    mode: "capture",
    capture: { messagesPerTopic: 50, window: "PT2H" },
    roles: [
      {
        name: "sensors",
        serviceAccounts: ["sensor-1", "sensor-2"],
        publish: ["sensors/${clientid}/#"],
      },
    ],
  },
  status: { phase },
});

interface World {
  persona: Persona;
  enabled: boolean;
  allowanceFails: boolean;
  measured: boolean;
  brokers: ReturnType<typeof broker>[];
}

function allowance(world: World) {
  return {
    enabled: world.enabled,
    origin: world.enabled ? "project" : "default",
    raise: `an org admin can raise this in Organization settings > Projects > ${PROJECT}`,
    modes: world.enabled ? ["ingest", "capture"] : [],
    addresses: world.enabled ? ["platform"] : [],
    ports: [],
    brokers: meter(world.enabled ? 2 : 0, world.brokers.length),
    connections: meter(world.enabled ? 1000 : 0, world.brokers.length * 2),
    messagesPerSecond: meter(world.enabled ? 10000 : 0, 0),
    maxMessageBytes: { limit: world.enabled ? 65536 : 0 },
    portsInUse: [],
  };
}

function measured(world: World, name: string) {
  if (!world.measured)
    return {
      broker: name,
      measured: false,
      reason: "the broker did not answer within 2 s",
      accounts: [],
      rates: [],
      refusedPublishes: null,
    };
  return {
    broker: name,
    measured: true,
    publishers: 1,
    messagesPerSecond: 0.2,
    accounts: [
      {
        serviceAccount: "sensor-1",
        connected: 1,
        since: "2026-10-24T19:40:00Z",
      },
    ],
    rates: [
      { at: "2026-10-24T19:44:50Z", perSecond: 0.2 },
      { at: "2026-10-24T19:44:55Z", perSecond: 0.2 },
    ],
    refusedPublishes: 0,
  };
}

async function stubApi(page: Page, world: World): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me")) {
      return json({
        subject: "c1",
        username: `demo.${world.persona}`,
        name: `Demo ${world.persona}`,
        roles: ["portal-editor"],
      });
    }
    if (path === "/api/v1/projects")
      return json({
        apiVersion: API,
        kind: "ProjectList",
        items: [{ name: PROJECT }],
      });
    if (path.endsWith("/permissions/me")) {
      const kinds = [
        "MqttBroker",
        "ServiceAccount",
        "DataSource",
        "Pipeline",
        "Endpoint",
        "Policy",
      ];
      return json({
        project: PROJECT,
        bootstrap: false,
        grants: [
          {
            role: world.persona,
            binding: world.persona,
            rule: { kinds, verbs: VERBS[world.persona] },
          },
        ],
      });
    }
    if (path.endsWith("/mqtt/allowance")) {
      if (world.allowanceFails)
        return json(
          {
            type: "about:blank",
            title: "Bad Gateway",
            status: 502,
            detail: "the allowance could not be read",
          },
          502,
        );
      return json(allowance(world));
    }
    const measuredPath = /\/mqttbrokers\/([^/]+)\/measured$/.exec(path);
    if (measuredPath) return json(measured(world, measuredPath[1]));
    const capturedPath = /\/mqttbrokers\/([^/]+)\/captured$/.exec(path);
    if (capturedPath) {
      return json({ broker: capturedPath[1], mode: "ingest", window: "PT1H", messagesPerTopic: 20, messages: [], truncated: false });
    }
    const draft = /\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft) {
      // The form saves its draft before it checks (T-0769); the draft is not the change.
      if (request.method() !== "PUT") return route.fulfill({ status: 204 });
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({
        project: PROJECT,
        kind: draft[1],
        name: draft[2],
        manifest,
        touchedBy: world.persona,
        touchedKind: "person",
        version: 1,
        updatedAt: "2026-10-24T20:00:00Z",
      });
    }
    if (request.method() === "PUT" || request.method() === "POST") {
      const body = request.postDataJSON() as { kind?: string };
      if (body.kind === "MqttBroker" && world.brokers.length >= 2) {
        return json(
          {
            type: "about:blank",
            title: "Forbidden",
            status: 403,
            detail:
              "MQTT allowance: brokers 2 reached (2 in use); an org admin can raise this in Organization settings > Projects > helsinki",
          },
          403,
        );
      }
      return json({ valid: true });
    }
    if (path.endsWith("/access")) return json({ roles: [], outOfWindow: [] });
    if (/\/mqttbrokers\/[^/]+\/bindings$/.test(path)) return json({ broker: "sensors", bindings: [] });
    if (path.endsWith("/watch")) return json({ observedAt: "2026-10-29T20:15:00Z", refusals: [], refusalsRead: false });
    const one = /\/mqttbrokers\/([^/]+)$/.exec(path);
    const named = one ? world.brokers.find((b) => (b as { metadata: { name: string } }).metadata.name === one[1]) : undefined;
    if (named) return json(named);
    const plural = path.split("/").pop() ?? "";
    const items: Record<string, unknown[]> = {
      mqttbrokers: world.brokers,
      serviceaccounts: [
        {
          apiVersion: API,
          kind: "ServiceAccount",
          metadata: { name: "sensor-1", namespace: PROJECT },
          spec: {},
        },
      ],
    };
    return json({ apiVersion: API, kind: "List", items: items[plural] ?? [] });
  });
}

/** Navigates, waits for the page's first heading and returns the milliseconds that took. */
async function open(page: Page, path: string): Promise<number> {
  const started = Date.now();
  await page.goto(`${path}${path.includes("?") ? "&" : "?"}lang=en`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  return Date.now() - started;
}

async function record(
  page: Page,
  state: string,
  persona: Persona,
  ms?: number,
): Promise<void> {
  const axe = await axeViolations(page);
  // Only an enabled New counts: off and full keep it closed with the reason (T-5589).
  const newButtons = await page
    .getByRole("button", { name: /^New MQTT broker/ })
    .and(page.locator(':not([aria-disabled="true"])'))
    .count();
  if (SHOTS) {
    mkdirSync(SHOTS, { recursive: true });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      await page.screenshot({
        path: join(SHOTS, `mqtt-${state}-${persona}-${width}.png`),
        fullPage: true,
      });
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    appendFileSync(
      join(SHOTS, "critic-mqtt.jsonl"),
      `${JSON.stringify({ state, persona, ms, axe, newButtons })}\n`,
    );
  }
  expect(axe, `axe on ${state} as ${persona}`).toEqual([]);
}

const PAGE = `/projects/${PROJECT}/mqttbrokers`;

test.describe("UI critic: MQTT publishing", () => {
  test("off, empty, error, live broker and not measured, as each persona", async ({
    page,
  }) => {
    const world: World = {
      persona: "editor",
      enabled: false,
      allowanceFails: false,
      measured: true,
      brokers: [],
    };
    await stubApi(page, world);

    await record(page, "off", "editor", await open(page, PAGE));
    await expect(page.getByText("MQTT is off for this project", { exact: true })).toBeVisible();

    world.enabled = true;
    await record(page, "empty", "editor", await open(page, PAGE));
    await expect(page.locator('[data-allowance="on"]')).toBeVisible();

    world.allowanceFails = true;
    await record(page, "error", "editor", await open(page, PAGE));
    await expect(page.locator('[data-allowance="error"]')).toBeVisible();
    world.allowanceFails = false;

    world.brokers = [broker("sensors", "Ready")];
    for (const persona of ["steward", "editor", "viewer"] as const) {
      world.persona = persona;
      await record(page, "live", persona, await open(page, `${PAGE}/sensors`));
      await expect(page.locator('[data-measured="yes"]')).toBeVisible();
    }

    world.measured = false;
    world.persona = "steward";
    await record(page, "not-measured", "steward", await open(page, `${PAGE}/sensors`));
    await expect(page.locator('[data-measured="no"]')).toBeVisible();
  });

  test("the add form, the refusal at the limit and the MQTT data source", async ({
    page,
  }) => {
    const world: World = {
      persona: "editor",
      enabled: true,
      allowanceFails: false,
      measured: true,
      brokers: [broker("sensors", "Ready")],
    };
    await stubApi(page, world);

    await open(page, PAGE);
    await page.getByRole("button", { name: "New MQTT broker" }).first().click();
    await expect(
      page.getByRole("textbox", { name: /^Name/ }).first(),
    ).toBeVisible();
    await record(page, "add-form", "editor");

    world.brokers = [...world.brokers, broker("gates", "Ready")];
    await open(page, PAGE);
    await expect(page.locator("[data-allowance='full']")).toContainText(
      /All 2 brokers are in use: a new one is refused\./,
    );
    // New is closed at the limit; the form's own address still reaches the API's refusal.
    await expect(
      page.getByRole("button", { name: "New MQTT broker" }).first(),
    ).toHaveAttribute("aria-disabled", "true");
    await open(page, `${PAGE}/new`);
    await page.getByRole("textbox", { name: /^Name/ }).first().fill("vents");
    await page.getByRole("textbox", { name: /^Role/ }).first().fill("vents");
    // The role's device is picked from the project's accounts (T-5590).
    await page
      .getByRole("combobox", { name: /Service accounts/ })
      .first()
      .selectOption("sensor-1");
    await page.getByRole("button", { name: "Check", exact: true }).click();
    await expect(
      page.getByText(/MQTT allowance: brokers 2 reached/),
    ).toBeVisible();
    await record(page, "refused-limit", "editor");

    await open(page, `/projects/${PROJECT}/datasources`);
    await page.getByRole("button", { name: "New data source" }).first().click();
    await page.getByRole("combobox", { name: "Type" }).selectOption("mqtt");
    await expect(
      page.getByRole("combobox", { name: "Broker", exact: true }),
    ).toBeVisible();
    await record(page, "datasource", "editor");
  });
});
