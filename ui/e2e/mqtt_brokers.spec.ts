import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-3839: a project's MQTT brokers and who talks to whom, then an MQTT data source that reads
// one of them. `vite preview` has no Portal API behind it, so the browser answers it; the clicks,
// the routing and the rendering are the real ones. The camera publishing and the data arriving
// are the live journey of T-3838 on dev.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";

const item = (kind: string, name: string, spec: unknown, status?: unknown) => ({
  apiVersion: API,
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
  ...(status ? { status } : {}),
});

const LISTS: Record<string, unknown[]> = {
  mqttbrokers: [
    item(
      "MqttBroker",
      "cameras",
      { roles: [{ name: "cams", serviceAccounts: ["cam-1", "cam-2"], publish: ["cameras/${clientid}/frames"] }] },
      { phase: "Live" },
    ),
  ],
  datasources: [
    item("DataSource", "frames", { type: "mqtt", mqtt: { brokerRef: { kind: "MqttBroker", name: "cameras" }, topics: ["cameras/+/frames"] } }),
  ],
  pipelines: [
    item("Pipeline", "camera-frames", {
      class: "resident",
      source: { dataSourceRef: "frames" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:cams-in",
    }),
  ],
  endpoints: [item("Endpoint", "cams-in", {})],
  serviceaccounts: [item("ServiceAccount", "cam-1", {})],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    // One broker's own page (T-6103) reads it by name, with its panels' own reads.
    if (url.pathname.endsWith("/access")) return json({ roles: [], outOfWindow: [] });
    if (/\/mqttbrokers\/([^/]+)\/bindings$/.test(url.pathname)) return json({ broker: "cameras", bindings: [] });
    if (url.pathname.endsWith("/measured")) {
      return json({ measured: false, observedAt: "2026-10-29T20:15:00Z", reason: "not measured in this journey", accounts: [], rates: [] });
    }
    if (url.pathname.endsWith("/watch")) return json({ observedAt: "2026-10-29T20:15:00Z", refusals: [], refusalsRead: false });
    const one = /\/mqttbrokers\/([^/]+)$/.exec(url.pathname);
    const broker = one ? LISTS.mqttbrokers?.find((b) => (b as { metadata: { name: string } }).metadata.name === one[1]) : undefined;
    if (broker) return json(broker);
    const plural = url.pathname.split("/").pop() ?? "";
    return json({ apiVersion: API, kind: "List", items: LISTS[plural] ?? [] });
  });
}

test.describe("MQTT brokers", () => {
  test("a steward reads a broker's access and connections, then reads it from a new data source", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/home?lang=en`);
    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "MQTT brokers" }).click();

    await expect(page.getByRole("heading", { level: 1, name: "MQTT brokers" })).toBeVisible();
    // T-6103: the broker's settings live on its own page; the list keeps the data path.
    await page.getByRole("link", { name: "cameras", exact: true }).first().click();
    await expect(page.getByRole("heading", { level: 1, name: "cameras" })).toBeVisible();
    const cams = page.getByRole("group", { name: "cams" });
    await expect(cams.getByRole("list", { name: "Service accounts" })).toContainText("cam-1");
    await expect(cams.getByRole("list", { name: "Service accounts" })).toContainText("cam-2");
    await expect(page.getByText("helsinki-cam-2", { exact: true })).toBeVisible();

    // cam-2 is named by the role but not declared in the project: that edge is red, with its fix.
    await page.getByRole("link", { name: "MQTT brokers", exact: true }).first().click();
    const connections = page.getByRole("table", { name: "Connections" });
    const red = connections.locator('tr[data-edge="red"]');
    await expect(red).toHaveCount(1);
    await expect(red).toContainText("No service account cam-2 in this project");
    await expect(red.getByRole("link", { name: "Fix it" })).toHaveAttribute("href", `/projects/${PROJECT}/mqttbrokers/cameras`);
    await expect(connections.locator('tr[data-edge="green"]')).toHaveCount(4);
    expect(await axeViolations(page)).toEqual([]);

    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Data sources" }).click();
    await page.getByRole("button", { name: "New data source" }).first().click();
    await page.getByRole("combobox", { name: "Type" }).selectOption("mqtt");
    const broker = page.getByRole("combobox", { name: "Broker", exact: true });
    await expect(broker.getByRole("option", { name: "cameras · Live" })).toHaveCount(1);
    await broker.selectOption({ label: "cameras · Live" });
    await expect(broker.locator("option:checked")).toHaveText("cameras · Live");
  });

  // T-5590: a role's devices are picked from the project's ServiceAccounts the API lists for this
  // person; cam-2, named by the stored broker but not declared, is never offered.
  test("a steward adds a broker whose role picks its device from the project's accounts", async ({ page }) => {
    await stubApi(page);
    // A route added later answers first: this steward may create brokers here.
    await page.route("**/api/v1/projects/helsinki/permissions/me", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ project: PROJECT, bootstrap: true, grants: [] }) }),
    );
    // The form saves its shared draft as it is typed (AG-61); the draft comes back as written.
    await page.route("**/api/v1/projects/helsinki/drafts/MqttBroker/*", async (route) => {
      const manifest = (route.request().postDataJSON() as { manifest?: unknown } | null)?.manifest ?? null;
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ project: PROJECT, kind: "MqttBroker", name: "sensors", manifest, verdict: null, touchedBy: "aino.virtanen", touchedKind: "person", version: 1, updatedAt: "2026-10-25T03:00:00Z" }),
      });
    });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "New MQTT broker" })).toBeVisible();
    const account = page.locator("#root_roles_0_serviceAccounts_0");
    await expect(account.locator("option")).toHaveText(["Choose…", "cam-1"]);
    await page.locator("#root_name").fill("sensors");
    await page.locator("#root_roles_0_name").fill("sensors");
    await account.selectOption("cam-1");
    await expect(account.locator("option:checked")).toHaveText("cam-1");
    await expect(page.locator("#root_roles_0_serviceAccounts__description")).toContainText("E.g. vent-1.");
    expect(await axeViolations(page)).toEqual([]);
    const shots = process.env.T5590_SHOTS;
    if (shots) {
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.screenshot({ path: `${shots}/mqtt-add-broker-picked-${width}.png`, fullPage: true });
      }
    }
  });

  test("a steward reads that a custom hostname waits for DNS and where devices will connect (T-5874)", async ({ page }) => {
    await stubApi(page);
    await page.route("**/api/v1/projects/helsinki/mqttbrokers", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          apiVersion: API,
          kind: "List",
          items: [
            item(
              "MqttBroker",
              "cameras",
              {
                address: "customHostname",
                hostname: "mqtt.helsinki.example",
                roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/frames"] }],
              },
              {
                phase: "Pending",
                conditions: [{ type: "Ready", status: "False", reason: "WaitingForDns", message: "waiting for DNS" }],
              },
            ),
          ],
        }),
      }),
    );
    await page.route("**/api/v1/projects/helsinki/mqttbrokers/cameras", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(
          item(
            "MqttBroker",
            "cameras",
            {
              address: "customHostname",
              hostname: "mqtt.helsinki.example",
              roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/frames"] }],
            },
            { phase: "Pending", conditions: [{ type: "Ready", status: "False", reason: "WaitingForDns", message: "waiting for DNS" }] },
          ),
        ),
      }),
    );
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
    const notice = page.locator("[data-dns='waiting']");
    await expect(notice).toContainText("Waiting for DNS");
    await expect(notice).toContainText("Point a CNAME record from mqtt.helsinki.example");
    await expect(page.getByText("mqtt.helsinki.example:8883", { exact: true }).first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    const shots = process.env.T5874_SHOTS;
    if (shots) {
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await notice.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${shots}/mqtt-hostname-waiting-for-dns-${width}.png`, fullPage: true });
      }
    }
  });

  test("a steward fences a broker to the city's networks and a bad range is named at the field (PF-129)", async ({ page }) => {
    await stubApi(page);
    await page.route("**/api/v1/projects/helsinki/permissions/me", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ project: PROJECT, bootstrap: true, grants: [] }) }),
    );
    await page.route("**/api/v1/projects/helsinki/drafts/MqttBroker/*", async (route) => {
      const manifest = (route.request().postDataJSON() as { manifest?: unknown } | null)?.manifest ?? null;
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ project: PROJECT, kind: "MqttBroker", name: "sensors", manifest, verdict: null, touchedBy: "aino.virtanen", touchedKind: "person", version: 1, updatedAt: "2026-10-27T18:00:00Z" }),
      });
    });
    const shots = process.env.T5870_SHOTS;
    const shoot = async (state: string) => {
      if (!shots) return;
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await page.locator("#root_allowedSources__description").scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${shots}/mqtt-broker-sources-${state}-${width}.png` });
      }
      await page.setViewportSize({ width: 1440, height: 900 });
    };
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "New MQTT broker" })).toBeVisible();
    await page.locator("#root_name").fill("sensors");
    await page.locator("#root_roles_0_name").fill("sensors");
    await page.locator("#root_roles_0_serviceAccounts_0").selectOption("cam-1");
    const help = page.locator("#root_allowedSources__description");
    await expect(help).toContainText("E.g. 203.0.113.0/24");
    await shoot("empty");
    await help.locator("xpath=following::button[normalize-space()='Add'][1]").click();
    const range = page.locator("#root_allowedSources_0");
    await range.fill("sensors.city.example/24");
    await range.blur();
    await expect(page.getByText(/must match pattern|does not match|format/i).first()).toBeVisible();
    await shoot("error");
    await range.fill("203.0.113.0/24");
    await range.blur();
    await expect(page.getByText(/must match pattern|does not match|format/i)).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shoot("filled");
  });

  test("a steward raises the connections one address may hold for a NAT'd fleet (T-5888)", async ({ page }) => {
    await stubApi(page);
    await page.route("**/api/v1/projects/helsinki/permissions/me", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ project: PROJECT, bootstrap: true, grants: [] }) }),
    );
    const drafted: unknown[] = [];
    await page.route("**/api/v1/projects/helsinki/drafts/MqttBroker/*", async (route) => {
      const manifest = (route.request().postDataJSON() as { manifest?: unknown } | null)?.manifest ?? null;
      drafted.push(manifest);
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ project: PROJECT, kind: "MqttBroker", name: "sensors", manifest, verdict: null, touchedBy: "aino.virtanen", touchedKind: "person", version: 1, updatedAt: "2026-10-28T05:00:00Z" }),
      });
    });
    const shots = process.env.T5888_SHOTS;
    const field = page.locator("#root_limits_connectionsPerAddress");
    const shoot = async (state: string) => {
      if (!shots) return;
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await field.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${shots}/mqtt-broker-per-address-${state}-${width}.png` });
      }
      await page.setViewportSize({ width: 1440, height: 900 });
    };
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "New MQTT broker" })).toBeVisible();
    await page.locator("#root_name").fill("sensors");
    await page.locator("#root_roles_0_name").fill("sensors");
    await page.locator("#root_roles_0_serviceAccounts_0").selectOption("cam-1");
    // The limits sit in a folded group: a steward opens it to raise one.
    await page.getByRole("group", { name: "Limits and port" }).getByText("Limits and port", { exact: true }).click();
    await expect(page.getByLabel("Connections per source address")).toBeVisible();
    await expect(page.locator("#root_limits_connectionsPerAddress__description")).toContainText("Empty is 20. E.g. 50.");
    await shoot("empty");
    await field.fill("0");
    await field.blur();
    await expect(field).toHaveAttribute("aria-invalid", "true");
    await expect(page.locator("#root_limits_connectionsPerAddress__error")).toBeVisible();
    await shoot("error");
    await field.fill("50");
    await field.blur();
    await expect(page.locator("#root_limits_connectionsPerAddress__error")).toHaveCount(0);
    await expect
      .poll(() => JSON.stringify(drafted.at(-1) ?? null))
      .toContain('"connectionsPerAddress":50');
    expect(await axeViolations(page)).toEqual([]);
    await shoot("filled");
  });

  test("the broker list shows the networks a broker admits, or any address", async ({ page }) => {
    LISTS.mqttbrokers.push(
      item(
        "MqttBroker",
        "fenced",
        { roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/frames"] }], allowedSources: ["203.0.113.0/24", "2001:db8::/48"] },
        { phase: "Live" },
      ),
    );
    try {
      await stubApi(page);
      await page.setViewportSize({ width: 1440, height: 900 });
      // T-6103: each broker's Networks section, on its own page.
      await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
      await expect(page.getByTestId("broker-sources")).toContainText("Any address with a valid token");
      await page.goto(`/projects/${PROJECT}/mqttbrokers/fenced?lang=en`);
      const sources = page.getByTestId("broker-sources");
      await expect(sources.getByText("203.0.113.0/24", { exact: true })).toBeVisible();
      await expect(sources.getByText("2001:db8::/48", { exact: true })).toBeVisible();
      const shots = process.env.T5870_SHOTS;
      if (shots) {
        for (const width of [1440, 400]) {
          await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
          await sources.scrollIntoViewIfNeeded();
          await page.screenshot({ path: `${shots}/mqtt-broker-sources-list-${width}.png` });
        }
      }
    } finally {
      LISTS.mqttbrokers.pop();
    }
  });

  test("a steward who reads no account is told where one comes from", async ({ page }) => {
    await stubApi(page);
    await page.route("**/api/v1/projects/helsinki/permissions/me", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ project: PROJECT, bootstrap: true, grants: [] }) }),
    );
    await page.route("**/api/v1/projects/helsinki/serviceaccounts", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ apiVersion: API, kind: "List", items: [] }) }),
    );
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/new?lang=en`);
    const hint = page.locator("#root_roles_0_serviceAccounts_0-empty");
    await expect(hint).toContainText("Add a device from the broker's page");
    expect(await axeViolations(page)).toEqual([]);
    const shots = process.env.T5590_SHOTS;
    if (shots) {
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await hint.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `${shots}/mqtt-add-broker-no-accounts-${width}.png` });
      }
    }
  });
});

// T-6103: one broker's own page over the reads the Portal makes, in each state the owner named:
// receiving, idle, no traffic yet, and a caller who may not read the broker. Shots go to
// T6103_SHOTS at 1440 and 400 px.
test.describe("one MQTT broker's page (T-6103)", () => {
  const MEASURED = {
    measured: true,
    observedAt: "2026-10-29T20:15:00Z",
    messagesPerSecond: 6.2,
    publishers: 1,
    reason: null,
    accounts: [
      { serviceAccount: "cam-1", connected: 1, since: "2026-10-29T17:03:00Z" },
      { serviceAccount: "cam-2", connected: 0, since: null },
    ],
    rates: Array.from({ length: 59 }, (_, i) => ({
      at: new Date(Date.UTC(2026, 9, 29, 20, 10, 5 + i * 5)).toISOString(),
      perSecond: Math.round(60 + 15 * Math.sin(i / 4)) / 10,
    })),
    refusedPublishes: 812,
  };
  const WATCHED = {
    observedAt: "2026-10-29T20:15:00Z",
    refusals: [
      { at: "2026-10-29T19:00:00Z", reason: "expired", account: "cam-2" },
      { at: "2026-10-29T19:05:00Z", reason: "expired", account: "cam-2" },
    ],
    refusalsRead: true,
    connected: null,
    messagesPerSecond: null,
    reason: null,
  };

  // The page's world agrees with itself: both accounts the role lists exist, cam-3 holds the role
  // by a RoleBinding, and the gateway's answer per account says the same (T-6103 review).
  const ACCOUNTS = [
    item("ServiceAccount", "cam-1", {}),
    item("ServiceAccount", "cam-2", {}),
    item("ServiceAccount", "cam-3", { roles: [{ role: "mqtt/cameras/cams", scope: { project: PROJECT } }] }),
    item("ServiceAccount", "cam-4", {}),
  ];
  const STEWARD = { project: PROJECT, bootstrap: true, grants: [] };
  const READER = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount", "DataSource", "Pipeline", "Endpoint", "Policy"], verbs: ["read"] } }] };

  async function openBroker(page: Page, measured: unknown, refused = false, permissions: unknown = STEWARD): Promise<void> {
    await stubApi(page);
    await page.route("**/api/v1/projects/helsinki/permissions/me", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify(permissions) }),
    );
    await page.route("**/api/v1/projects/helsinki/serviceaccounts", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ apiVersion: API, kind: "List", items: ACCOUNTS }) }),
    );
    await page.route("**/api/v1/projects/helsinki/mqttbrokers/cameras**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
      if (refused && path.endsWith("/cameras")) {
        return json({ type: "about:blank", title: "Not Found", status: 404, detail: "MQTT broker 'cameras' not found in project 'helsinki'" }, 404);
      }
      if (path.endsWith("/measured")) return json(measured);
      if (path.endsWith("/watch")) return json(WATCHED);
      if (path.endsWith("/access")) {
        const account = new URL(route.request().url()).searchParams.get("account") ?? "";
        const held = ["cam-1", "cam-2", "cam-3"].includes(account);
        return json({
          roles: held ? [{ role: "cams", publish: ["cameras/${clientid}/frames"], subscribe: [], bound: account === "cam-3", legacy: account !== "cam-3" }] : [],
          outOfWindow: [],
        });
      }
      if (path.endsWith("/bindings")) {
        // The same edge the data path draws: data source frames reads cameras/+/frames for camera-frames.
        return json({
          broker: "cameras",
          bindings: [
            {
              topicFilter: "cameras/+/frames",
              dataSource: { kind: "DataSource", name: "frames" },
              pipeline: { kind: "Pipeline", name: "camera-frames" },
              endpoint: { kind: "Endpoint", name: "cams-in", space: "cameras" },
              type: "Device",
              attributes: [{ from: "frame", to: "image" }],
            },
          ],
        });
      }
      if (path.endsWith("/devices")) {
        return json({
          mayAdd: permissions === STEWARD,
          roles: [{ name: "cams", publish: ["cameras/${clientid}/frames"] }],
          tokenUrl: "https://id.dev.joinedcontext.com/realms/jc/protocol/openid-connect/token",
          issuer: "https://id.dev.joinedcontext.com/realms/jc",
        });
      }
      if (path.endsWith("/cameras")) return json(LISTS.mqttbrokers?.[0]);
      return json({ apiVersion: API, kind: "List", items: [] });
    });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
  }

  async function shoot(page: Page, state: string): Promise<void> {
    expect(await axeViolations(page), `axe on ${state}`).toEqual([]);
    await page.setViewportSize({ width: 400, height: 900 });
    // Only a table scrolls inside its own box; the page itself never scrolls sideways at 400 px.
    expect(await page.evaluate(() => document.documentElement.scrollWidth), `width of ${state} at 400 px`).toBeLessThanOrEqual(400);
    const shots = process.env.T6103_SHOTS;
    if (!shots) return;
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      // The viewport takes the page's whole height, so the sticky proposal bar sits where it
      // stands at the foot of the editor; a fullPage capture paints it at the first fold instead.
      const height = await page.evaluate(() => document.documentElement.scrollHeight);
      await page.setViewportSize({ width, height });
      await page.evaluate(() => window.scrollTo(0, 0));
      // Two frames: the resize's layout and its paint land before the capture.
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      await page.screenshot({ path: `${shots}/broker-${state}-${width}.png` });
      if (state === "editing") {
        // As a person sees it while editing: scrolled into the roles, the bar held at the viewport's foot.
        await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
        await page.locator("#broker-access").scrollIntoViewIfNeeded();
        await expect(page.getByRole("region", { name: "Pending changes" })).toBeInViewport();
        await page.screenshot({ path: `${shots}/broker-editing-scrolled-${width}.png` });
      }
    }
    await page.setViewportSize({ width: 1440, height: 900 });
  }

  test("receiving: live figures, the chart and each account's connections", async ({ page }) => {
    await openBroker(page, MEASURED);
    await expect(page.getByRole("heading", { level: 1, name: "cameras" })).toBeVisible();
    await expect(page.locator("[data-state='receiving']")).toHaveText("Receiving");
    await expect(page.getByTestId("live-strip")).toHaveText("6.2 messages/s · 1 connected · 2 refused sign-ins today");
    await expect(page.getByRole("img", { name: /Messages per second, last 5 min/ })).toBeVisible();
    await expect(page.getByRole("table", { name: "Device accounts and their connections now" })).toContainText("1 connected");
    await expect(page.getByRole("list", { name: "Highest QoS" })).toContainText("QoS 1 · highest");
    // No panel contradicts another: the role lists both accounts, both exist, and the gateway agrees.
    await expect(page.getByText(/holds no role|No service account|No topic is bound|You may not add devices/)).toHaveCount(0);
    await page.getByRole("link", { name: "Limits" }).click();
    await expect(page.locator("#broker-limits")).toBeInViewport();
    await shoot(page, "receiving");
  });

  test("idle, and no traffic yet", async ({ page }) => {
    const quiet = MEASURED.rates.map((point) => ({ ...point, perSecond: 0 }));
    await openBroker(page, { ...MEASURED, messagesPerSecond: 0, rates: quiet });
    await expect(page.locator("[data-state='idle']")).toHaveText("Idle");
    await expect(page.locator("[data-chart='quiet']")).toContainText("No message arrived in the last 5 min");
    await shoot(page, "idle");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    const offline = MEASURED.accounts.map((account) => ({ ...account, connected: 0, since: null }));
    await openBroker(page, { ...MEASURED, messagesPerSecond: 0, accounts: offline, rates: [], refusedPublishes: 0 });
    await expect(page.getByText("The chart starts once the broker has kept two samples")).toBeVisible();
    await expect(page.getByRole("table", { name: "Device accounts and their connections now" })).not.toContainText("connected");
    await shoot(page, "no-traffic");
  });

  test("a steward edits in place and sees the pending changes, a reader sees them read only", async ({ page }) => {
    await openBroker(page, MEASURED);
    const cams = page.getByRole("group", { name: "cams" });
    await cams.getByRole("combobox", { name: "Add an account to role cams" }).selectOption("cam-4");
    const publish = cams.getByRole("textbox", { name: "Publish" });
    await publish.fill("cameras/+/frames");
    await expect(cams.getByText("Put a ${clientid} level before any wildcard")).toBeVisible();
    await page.getByRole("spinbutton", { name: "Messages per second, per device" }).fill("20");
    const bar = page.getByRole("region", { name: "Pending changes" });
    await expect(bar).toContainText("cam-4 gets role cams");
    await expect(bar).toContainText("Messages per second per device: 10 → 20");
    await expect(page.getByRole("navigation", { name: "Sections of this broker" }).getByRole("img", { name: "Not proposed" })).toHaveCount(2);
    await shoot(page, "editing");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await openBroker(page, MEASURED, false, READER);
    await expect(page.getByText("Read only", { exact: true })).toBeVisible();
    await expect(page.getByRole("spinbutton", { name: "Messages per second, per device" })).toBeDisabled();
    await shoot(page, "read-only");
  });

  test("a caller who may not read the broker sees the refusal and no settings", async ({ page }) => {
    await openBroker(page, MEASURED, true, { project: PROJECT, grants: [] });
    await expect(page.getByRole("link", { name: "← MQTT brokers" })).toBeVisible();
    await expect(page.getByText("Topic access of cameras by role")).toHaveCount(0);
    await expect(page.getByText("Your role may not read MQTT brokers here")).toBeVisible();
    await shoot(page, "no-permission");
  });
});
