/**
 * T-3839: the MQTT brokers page (MF-54, ADR-N-048). Each broker's topic access by role, how a
 * device connects (never a secret), and the edges from a device to the Policy, a broken one red
 * with a link to where it is fixed.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { MqttBrokersPage, brokerAddress } from "../src/pages/mqttbrokers/MqttBrokersPage";
import { connectionsOf, urnName } from "../src/pages/mqttbrokers/connections";
import type { Resources } from "../src/pages/mqttbrokers/connections";
import { fromMqttBrokerManifest, mqttBrokerSchema, toMqttBrokerManifest } from "../src/schemas/mqttbroker";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "espoo";
const API = "joinedcontext.com/v1alpha1";

const item = (kind: string, name: string, spec: unknown, status?: unknown) => ({
  apiVersion: API,
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
  ...(status ? { status } : {}),
});

const CAMERAS = item(
  "MqttBroker",
  "cameras",
  {
    roles: [
      { name: "cams", serviceAccounts: ["cam-1", "cam-2"], publish: ["cameras/${clientid}/frames"] },
      { name: "ops", serviceAccounts: ["cam-1"], subscribe: ["cameras/${clientid}/commands"] },
    ],
  },
  { phase: "Live" },
);

function world(): Resources {
  return {
    project: PROJECT,
    brokers: [CAMERAS],
    dataSources: [
      item("DataSource", "frames", { type: "mqtt", mqtt: { brokerRef: { kind: "MqttBroker", name: "cameras" }, topics: ["cameras/+/frames"] } }),
      item("DataSource", "orphan", { type: "mqtt", mqtt: { brokerRef: { kind: "MqttBroker", name: "gone" }, topics: ["x/#"] } }),
      item("DataSource", "external", { type: "mqtt", mqtt: { urls: ["tls://m:8883"], topics: ["y"] } }),
    ],
    pipelines: [
      item("Pipeline", "camera-frames", {
        source: { dataSourceRef: "frames" },
        targetEndpoint: "urn:ngsi-ld:Endpoint:espoo.fi:espoo:cams-in",
      }),
      item("Pipeline", "elsewhere", {
        sources: [{ dataSourceRef: { kind: "DataSource", name: "frames" } }],
        outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:espoo.fi:espoo:missing" }],
      }),
    ],
    endpoints: [item("Endpoint", "cams-in", { policyRef: "urn:ngsi-ld:Policy:espoo.fi:espoo:cams-write" })],
    policies: [],
    accounts: [item("ServiceAccount", "cam-1", {})],
  } as unknown as Resources;
}

/** The accounts the broker page reads: cam-1 listed on the role, cam-3 bound by a RoleBinding (T-5872). */
const ACCOUNTS = [
  item("ServiceAccount", "cam-1", {}),
  item("ServiceAccount", "cam-2", {}),
  item("ServiceAccount", "cam-3", { roles: [{ role: "mqtt/cameras/cams", scope: { project: PROJECT } }] }),
  item("ServiceAccount", "cam-4", {}),
];
const READS = ["MqttBroker", "ServiceAccount", "DataSource", "Pipeline", "Endpoint", "Policy"];
const EDITOR = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read", "propose"] } }, { rule: { kinds: READS, verbs: ["read"] } }] };
const READER = { project: PROJECT, grants: [{ rule: { kinds: READS, verbs: ["read"] } }] };

describe("the connections around a project's brokers", () => {
  it("names each edge with what allows it and turns the broken ones red with their fix", () => {
    const edges = connectionsOf(world());
    const red = edges.filter((edge) => edge.problem).map((edge) => [edge.kind, edge.problem?.key, edge.fix]);
    expect(red).toEqual([
      ["device", "mqttbrokers.connections.unknownAccount", { plural: "mqttbrokers", name: "cameras" }],
      ["policy", "mqttbrokers.connections.noPolicy", { plural: "endpoints", name: "cams-in" }],
      ["endpoint", "mqttbrokers.connections.noEndpoint", { plural: "pipelines", name: "elsewhere" }],
      ["broker", "mqttbrokers.connections.noBroker", { plural: "datasources", name: "orphan" }],
      ["pipeline", "mqttbrokers.connections.unread", { plural: "pipelines", name: "" }],
    ]);
    const device = edges.find((edge) => edge.kind === "device" && edge.from === "cam-1");
    expect(device?.allowedBy.values).toEqual({ role: "cams", client: "espoo-cam-1" });
    expect(edges.some((edge) => edge.from === "external" || edge.to === "external")).toBe(false);
  });

  it("is all green when every reference is there, and judges no Endpoint without a policyRef", () => {
    const green = world();
    green.dataSources = green.dataSources.slice(0, 1);
    green.pipelines = green.pipelines.slice(0, 1);
    green.accounts.push(item("ServiceAccount", "cam-2", {}) as never);
    green.policies = [item("Policy", "cams-write", {}) as never];
    expect(connectionsOf(green).filter((edge) => edge.problem)).toEqual([]);
    green.endpoints = [item("Endpoint", "cams-in", {}) as never];
    expect(connectionsOf(green).some((edge) => edge.kind === "policy")).toBe(false);
  });

  it("says a broker in error, and an Endpoint the project does not hold whatever its URN reads", () => {
    const failing = world();
    failing.brokers = [{ ...CAMERAS, status: { phase: "Error" } } as never];
    failing.pipelines[1] = item("Pipeline", "elsewhere", {
      sources: [{ dataSourceRef: "frames" }],
      outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:vantaa.fi:vantaa:shared" }],
    }) as never;
    const edges = connectionsOf(failing);
    expect(edges.find((edge) => edge.kind === "broker" && edge.to === "frames")?.problem?.key).toBe(
      "mqttbrokers.connections.brokerError",
    );
    expect(edges.find((edge) => edge.to === "shared")?.problem?.key).toBe("mqttbrokers.connections.noEndpoint");
  });

  it("reads an Endpoint URN for its name alone and refuses what is not one (ADR-N-041)", () => {
    expect(urnName("urn:ngsi-ld:Endpoint:espoo.fi:espoo:cams-in")).toBe("cams-in");
    expect(urnName("urn:ngsi-ld:Endpoint:cams-in")).toBe("cams-in");
    expect(urnName("urn:ngsi-ld:Endpoint:a:")).toBeUndefined();
    expect(urnName("https://example.org")).toBeUndefined();
    expect(urnName(undefined)).toBeUndefined();
  });
});

describe("the broker form", () => {
  it("writes the roles it was given and keeps the metadata it has no field for", () => {
    const manifest = toMqttBrokerManifest(
      PROJECT,
      {
        name: "cameras",
        roles: [{ name: "cams", serviceAccounts: [" cam-1 ", "", undefined as never], publish: ["cameras/${clientid}/frames"], subscribe: [] }],
        limits: { messagesPerSecond: 20 },
      },
      { metadata: { name: "cameras", labels: { team: "a" } } },
    ) as { metadata: Record<string, unknown>; spec: unknown };
    expect(manifest.metadata).toEqual({ name: "cameras", namespace: PROJECT, labels: { team: "a" } });
    expect(manifest.spec).toEqual({
      roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/frames"] }],
      limits: { messagesPerSecond: 20 },
    });
    expect(fromMqttBrokerManifest(CAMERAS).roles?.[1]).toEqual({
      name: "ops",
      serviceAccounts: ["cam-1"],
      publish: [],
      subscribe: ["cameras/${clientid}/commands"],
    });
  });

  it("refuses at the field a device subscription outside its own client id, and a port out of range", () => {
    const schema = mqttBrokerSchema((key) => key) as {
      properties: { roles: { items: { properties: { subscribe: { items: { pattern: string } } } } }; port: { minimum: number; maximum: number } };
    };
    const pattern = new RegExp(schema.properties.roles.items.properties.subscribe.items.pattern);
    expect(pattern.test("cameras/${clientid}/commands")).toBe(true);
    expect(pattern.test("cameras/+/${clientid}")).toBe(false);
    expect(pattern.test("#")).toBe(false);
    expect([schema.properties.port.minimum, schema.properties.port.maximum]).toEqual([18830, 18849]);
  });

  it("takes allowedSources as CIDR ranges at the field and carries them to the manifest and back (PF-129)", () => {
    const schema = mqttBrokerSchema((key) => key) as {
      properties: { allowedSources: { maxItems: number; items: { pattern: string } } };
    };
    const range = new RegExp(schema.properties.allowedSources.items.pattern);
    for (const ok of ["203.0.113.0/24", "198.51.100.7/32", "2001:db8::/48", "::ffff:203.0.113.0/120"]) {
      expect(range.test(ok)).toBe(true);
    }
    for (const bad of ["203.0.113.0", "sensors.city.example/24", "203.0.113.0/33", "256.1.1.1/8", "2001:db8::/129", ""]) {
      expect(range.test(bad)).toBe(false);
    }
    expect(schema.properties.allowedSources.maxItems).toBe(32);
    const manifest = toMqttBrokerManifest("espoo", {
      name: "cameras",
      roles: [{ name: "camera", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/#"] }],
      allowedSources: [" 203.0.113.0/24 ", undefined as unknown as string, ""],
    }) as { spec: { allowedSources?: string[] } };
    expect(manifest.spec.allowedSources).toEqual(["203.0.113.0/24"]);
    expect(fromMqttBrokerManifest(manifest).allowedSources).toEqual(["203.0.113.0/24"]);
    const open = toMqttBrokerManifest("espoo", { name: "cameras", roles: [], allowedSources: [] }) as { spec: object };
    expect(open.spec).not.toHaveProperty("allowedSources");
  });

  it("takes connectionsPerAddress in 1..10000 and carries it to the manifest and back (T-5888)", () => {
    const schema = mqttBrokerSchema((key) => key) as {
      properties: { limits: { properties: { connectionsPerAddress: { minimum: number; maximum: number } } } };
    };
    const field = schema.properties.limits.properties.connectionsPerAddress;
    expect([field.minimum, field.maximum]).toEqual([1, 10_000]);
    const manifest = toMqttBrokerManifest("espoo", {
      name: "cameras",
      roles: [{ name: "camera", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/#"] }],
      limits: { connectionsPerAddress: 50 },
    }) as { spec: { limits?: object } };
    expect(manifest.spec.limits).toEqual({ connectionsPerAddress: 50 });
    expect(fromMqttBrokerManifest(manifest).limits).toEqual({ connectionsPerAddress: 50 });
  });
});

describe("the broker's address", () => {
  it("is the broker's name under the project and the Portal's domain, on 8883 or its own port", () => {
    expect(brokerAddress("cameras", "espoo", "portal.dev.joinedcontext.com")).toBe("cameras.espoo.mqtt.dev.joinedcontext.com:8883");
    expect(brokerAddress("cameras", "espoo", "portal.example.org", 18830)).toBe("cameras.espoo.mqtt.example.org:18830");
    expect(brokerAddress("cameras", "espoo", "localhost")).toBeUndefined();
  });

  it("is the city's own name on 8883 for a custom hostname, wherever the Portal runs (T-5874)", () => {
    expect(brokerAddress("cameras", "espoo", "localhost", undefined, "mqtt.espoo.example")).toBe("mqtt.espoo.example:8883");
  });
});

describe("the MQTT brokers page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/mqttbrokers`);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  const lists = (): Record<string, unknown[]> => {
    const resources = world();
    return {
      mqttbrokers: resources.brokers,
      datasources: resources.dataSources,
      pipelines: resources.pipelines,
      endpoints: resources.endpoints,
      policies: resources.policies,
      serviceaccounts: resources.accounts,
    };
  };

  it("lists the brokers, each opening its own page, and turns a broken edge red with its fix", async () => {
    const all = lists();
    renderPage(<MqttBrokersPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/mqttbrokers`,
      answer: (url) => {
        const plural = url.pathname.split("/").pop() ?? "";
        return all[plural] ? json(list(all[plural])) : undefined;
      },
    });
    const orphan = await screen.findByText("No broker gone in this project");
    const row = orphan.closest("tr") as HTMLElement;
    expect(row.closest("table")?.querySelector("caption")?.textContent).toBe(en.mqttbrokers.connections.title);
    expect(row.dataset.edge).toBe("red");
    expect(within(row).getByText(en.mqttbrokers.connections.fix).closest("a")).toHaveAttribute(
      "href",
      `/projects/${PROJECT}/datasources/orphan/edit`,
    );
    // T-6103: a broker has a page of its own; the list no longer repeats its settings.
    expect(screen.queryByText("cam-1, cam-2")).toBeNull();
    expect(document.body.textContent).not.toMatch(/password\s*[:=]|secret\s*[:=]/i);
  });
});

/** The editor is live once the permissions and the accounts are read. */
async function editable(): Promise<HTMLElement> {
  const cams = await screen.findByRole("group", { name: "cams" });
  await waitFor(() => expect(within(cams).getByRole("combobox", { name: "Add an account to role cams" })).toBeInTheDocument());
  return cams;
}

/** One broker's page (T-6103) over stubbed reads: the broker, its lists, measured and watch. */
function showBroker(
  options: {
    broker?: unknown;
    measured?: () => Response;
    watch?: () => Response;
    brokerAnswer?: () => Response;
    permissions?: unknown;
    sent?: { method: string; path: string; body: unknown }[];
  } = {},
) {
  const resources = world();
  const all: Record<string, unknown[]> = {
    datasources: resources.dataSources,
    pipelines: resources.pipelines,
    endpoints: resources.endpoints,
    policies: resources.policies,
    serviceaccounts: ACCOUNTS,
  };
  return renderPage(<MqttBrokerPage project={PROJECT} name="cameras" />, {
    path: `/projects/${PROJECT}/mqttbrokers/cameras`,
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(options.permissions ?? EDITOR);
      if (request.method !== "GET") {
        options.sent?.push({ method: request.method, path: `${url.pathname}${url.search}`, body: await request.json() });
        return url.searchParams.has("dryRun")
          ? json({ valid: true })
          : json({ apiVersion: API, kind: "Change", metadata: { name: "chg-1" }, status: { phase: "Proposed" } }, 202);
      }
      if (url.pathname.endsWith("/mqttbrokers/cameras/measured")) return options.measured?.();
      if (url.pathname.endsWith("/mqttbrokers/cameras/watch")) return options.watch?.();
      if (url.pathname.endsWith("/mqttbrokers/cameras/access")) {
        // As the gateway decides: cam-1 and cam-2 hold cams through the role's own list, cam-3 by binding.
        const account = url.searchParams.get("account");
        const held = ["cam-1", "cam-2", "cam-3"].includes(account ?? "");
        return json({
          roles: held ? [{ role: "cams", publish: ["cameras/${clientid}/frames"], subscribe: [], bound: account === "cam-3", legacy: account !== "cam-3" }] : [],
          outOfWindow: [],
        });
      }
      if (url.pathname.endsWith("/mqttbrokers/cameras")) return options.brokerAnswer?.() ?? json(options.broker ?? CAMERAS);
      const plural = url.pathname.split("/").pop() ?? "";
      return all[plural] ? json(list(all[plural])) : undefined;
    },
  });
}

const MEASURED = {
  measured: true,
  observedAt: "2026-10-12T08:47:00Z",
  messagesPerSecond: 2.5,
  publishers: 2,
  reason: null,
  accounts: [
    { serviceAccount: "cam-1", connected: 2, since: "2026-10-12T08:46:51Z" },
    { serviceAccount: "cam-2", connected: 0, since: null },
  ],
  rates: [
    { at: "2026-10-12T08:46:50Z", perSecond: 2 },
    { at: "2026-10-12T08:46:55Z", perSecond: 4 },
    { at: "2026-10-12T08:47:00Z", perSecond: 2.5 },
  ],
  refusedPublishes: 812,
};

const WATCHED = {
  observedAt: "2026-10-12T08:47:00Z",
  refusals: [
    { at: "2026-10-12T08:40:00Z", reason: "expired", account: "cam-1" },
    { at: "2026-10-12T08:41:00Z", reason: "expired", account: "cam-1" },
    { at: "2026-10-12T08:42:00Z", reason: "no_role", account: "cam-2" },
  ],
  refusalsRead: true,
  connected: null,
  messagesPerSecond: null,
  reason: null,
};

describe("one MQTT broker's page (T-6103)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/mqttbrokers/cameras`);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows each role's holders, how a device connects, and the data path, with no panel contradicting another", async () => {
    showBroker();
    const cams = await editable();
    const holders = within(cams).getByRole("list", { name: en.mqttbrokers.acl.accounts });
    expect(within(holders).getAllByRole("listitem").map((li) => li.textContent?.replace(/×$/, ""))).toEqual([
      "cam-1· listed on the role",
      "cam-2· listed on the role",
      "cam-3",
    ]);
    expect(within(cams).getByText("cameras/${clientid}/frames")).toBeInTheDocument();
    expect(screen.getByText("espoo-cam-1")).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.connect.token)).toBeInTheDocument();
    for (const section of Object.values(en.mqttbrokers.page.section)) {
      expect(screen.getByRole("link", { name: section })).toHaveAttribute("href", expect.stringMatching(/^#broker-/));
    }
    expect((await screen.findAllByText("camera-frames")).length).toBeGreaterThan(0);
    // Who can do what agrees with the role card: the first account holds cams.
    expect(await screen.findByRole("table", { name: /cam-1/ })).toHaveTextContent("cams");
    // Every account the roles name exists, so the data path shows no red account edge.
    expect(screen.queryByText(/No service account cam-/)).toBeNull();
    expect(document.body.textContent).not.toMatch(/password\s*[:=]|secret\s*[:=]/i);
  });

  it("reads the live tiles, the chart and each account's connections from the broker and the gateway", async () => {
    showBroker({ measured: () => json(MEASURED), watch: () => json(WATCHED) });
    const figures = await screen.findByText(en.mqttbrokers.page.figure.rate);
    const tiles = figures.closest("dl") as HTMLElement;
    expect(within(tiles).getByText("2.5")).toBeInTheDocument();
    expect(within(tiles).getByText("2 / 1,000")).toBeInTheDocument();
    expect(within(tiles).getByText("812")).toBeInTheDocument();
    expect(await within(tiles).findByText("3")).toBeInTheDocument();
    expect(within(tiles).getByText("Mostly: expired")).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.state.receiving)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /last 1 min: highest 4, now 2.5/ })).toBeInTheDocument();
    const devices = screen.getByRole("table", { name: en.mqttbrokers.page.devices });
    expect(within(devices).getByText("2 connected")).toBeInTheDocument();
    expect(within(devices).getByText(en.mqttbrokers.page.offlineNow)).toBeInTheDocument();
    expect(screen.getByTestId("live-strip").textContent).toBe("2.5 messages/s · 2 connected · 3 refused sign-ins today");
  });

  it("says idle when every kept sample is zero, instead of a flat line", async () => {
    const quiet = { ...MEASURED, messagesPerSecond: 0, rates: MEASURED.rates.map((point) => ({ ...point, perSecond: 0 })) };
    showBroker({ measured: () => json(quiet), watch: () => json(WATCHED) });
    expect(await screen.findByText(en.mqttbrokers.page.state.idle)).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.chartQuiet.replace("{minutes}", "1"))).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /highest/ })).toBeNull();
  });

  it("says idle with no traffic yet: no chart until two samples, and what to do without a device", async () => {
    const idle = { ...MEASURED, messagesPerSecond: 0, accounts: [], rates: [], refusedPublishes: 0 };
    showBroker({ measured: () => json(idle), watch: () => json({ ...WATCHED, refusals: [] }) });
    expect(await screen.findByText(en.mqttbrokers.page.state.idle)).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.chartEmpty)).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.noDevices)).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.figure.refusedSignInsNote)).toBeInTheDocument();
  });

  it("states the broker's highest QoS and what happens to an offline device, as checked on RMQTT 0.24", async () => {
    showBroker();
    const qos = await screen.findByRole("list", { name: en.mqttbrokers.edit.qos });
    expect(qos.closest("[data-qos]")).toHaveAttribute("data-qos", "1");
    expect(within(qos).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["QoS 0 · offered", "QoS 1 · highest", "QoS 2 · not offered"]);
    expect(screen.getByText(en.mqttbrokers.page.qosWords)).toBeInTheDocument();
    expect(screen.getByText(en.mqttbrokers.page.offlineWords)).toBeInTheDocument();
    // No networks named: the page warns that any address may connect.
    expect(screen.getByTestId("broker-sources").textContent).toContain(en.mqttbrokers.connect.anySource);
  });

  it("edits limits in place with their units and a use bar, and lists networks as chips", async () => {
    const limited = {
      ...CAMERAS,
      spec: { ...(CAMERAS as { spec: object }).spec, limits: { messagesPerSecond: 5, maxConnections: 20 }, allowedSources: ["192.0.2.0/24"] },
    };
    showBroker({ broker: limited, measured: () => json(MEASURED) });
    await editable();
    const rate = screen.getByLabelText(en.mqttbrokers.edit.limit.messagesPerSecond);
    expect(rate).toHaveValue(5);
    expect(screen.getByLabelText(en.mqttbrokers.edit.limit.maxConnections)).toHaveValue(20);
    expect(screen.getByLabelText(en.mqttbrokers.edit.limit.maxMessageBytes)).toHaveValue(65536);
    expect(screen.getAllByText("bytes").length).toBeGreaterThan(0);
    expect(screen.getByRole("meter", { name: "2 of 20 in use" })).toBeInTheDocument();
    expect(within(screen.getByTestId("broker-sources")).getByText("192.0.2.0/24")).toBeInTheDocument();
    fireEvent.change(rate, { target: { value: "5000" } });
    expect(screen.getByText("Write a whole number from 1 to 1,000.")).toBeInTheDocument();
  });

  it("checks topics and networks as they are typed and names what blocks the proposal", async () => {
    showBroker();
    const cams = await editable();
    const publish = within(cams).getByRole("textbox", { name: en.mqttbrokers.acl.publish });
    for (const [typed, why] of [
      ["/cameras/x", en.mqttbrokers.edit.topic.emptyLevel],
      ["cameras/#/x", en.mqttbrokers.edit.topic.hash],
      ["cameras/a+/x", en.mqttbrokers.edit.topic.plus],
      ["#", en.mqttbrokers.edit.topic.everything],
      ["cameras/+/x", en.mqttbrokers.edit.topic.publishOwn],
    ]) {
      fireEvent.change(publish, { target: { value: typed } });
      expect(within(cams).getByText(why as string)).toBeInTheDocument();
    }
    const subscribe = within(cams).getByRole("textbox", { name: en.mqttbrokers.acl.subscribe });
    fireEvent.change(subscribe, { target: { value: "commands/#" } });
    expect(within(cams).getByText(en.mqttbrokers.edit.topic.subscribeOwn)).toBeInTheDocument();
    const sources = within(screen.getByTestId("broker-sources")).getByRole("textbox", { name: en.mqttbrokers.edit.sources });
    fireEvent.change(sources, { target: { value: "example.org" } });
    expect(screen.getByText(en.mqttbrokers.edit.topic.source.format)).toBeInTheDocument();
    // Sharing topics without a reason keeps Propose disabled, with the reason said.
    fireEvent.click(within(cams).getByLabelText(en.mqttbrokers.edit.shared));
    const propose = await screen.findByRole("button", { name: en.mqttbrokers.edit.propose });
    expect(propose).toHaveAttribute("aria-disabled", "true");
    expect(document.body.textContent).toContain("Role cams shares its topics: write why.");
  });

  it("proposes a role grant and a limit as one Change, the grant as a binding on the account", async () => {
    const sent: { method: string; path: string; body: unknown }[] = [];
    showBroker({ sent });
    const cams = await editable();
    fireEvent.change(within(cams).getByRole("combobox", { name: "Add an account to role cams" }), { target: { value: "cam-4" } });
    // The new account says on its chip that it waits for the proposal.
    expect(within(cams).getByText("cam-4").parentElement).toHaveTextContent(`cam-4· ${en.mqttbrokers.edit.unsaved}`);
    fireEvent.change(screen.getByLabelText(en.mqttbrokers.edit.limit.messagesPerSecond), { target: { value: "20" } });
    const bar = screen.getByRole("region", { name: en.mqttbrokers.edit.bar });
    expect(within(bar).getByText("Changes not proposed yet: 2")).toBeInTheDocument();
    expect(within(bar).getByText("cam-4 gets role cams")).toBeInTheDocument();
    expect(within(bar).getByText("Messages per second per device: 10 → 20")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Limits/ })).toContainHTML(en.mqttbrokers.edit.unsaved);
    fireEvent.click(within(bar).getByRole("button", { name: en.mqttbrokers.edit.propose }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent.map((call) => call.path)).toEqual([`/api/v1/projects/${PROJECT}/import?dryRun=All`, `/api/v1/projects/${PROJECT}/import`]);
    const manifests = (sent[1]?.body as { manifests: { kind: string; spec: Record<string, unknown> }[] }).manifests;
    const account = manifests.find((m) => m.kind === "ServiceAccount");
    expect(account?.spec.roles).toEqual([{ role: "mqtt/cameras/cams", scope: { project: PROJECT } }]);
    const broker = manifests.find((m) => m.kind === "MqttBroker");
    expect(broker?.spec.limits).toEqual({ messagesPerSecond: 20 });
    expect(await screen.findByText("chg-1")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: en.mqttbrokers.edit.bar })).toBeNull();
  });

  it("shows the settings read only, with the reason, to a reader who may not propose", async () => {
    showBroker({ permissions: READER });
    expect(await screen.findByText(en.mqttbrokers.edit.readOnlyTitle)).toBeInTheDocument();
    expect(screen.getByLabelText(en.mqttbrokers.edit.limit.messagesPerSecond)).toBeDisabled();
    expect(screen.getByRole("button", { name: en.mqttbrokers.edit.addRole })).toHaveAttribute("aria-disabled", "true");
    expect(screen.queryByRole("region", { name: en.mqttbrokers.edit.bar })).toBeNull();
  });

  const notFound = () =>
    new Response(JSON.stringify({ type: "about:blank", title: "Not Found", status: 404, detail: "MQTT broker 'cameras' not found in project 'espoo'" }), {
      status: 404,
      headers: { "content-type": "application/problem+json" },
    });

  it("shows the refusal and no settings to a caller who may not read the broker", async () => {
    showBroker({ brokerAnswer: notFound, permissions: { project: PROJECT, grants: [] } });
    expect(await screen.findByText(en.mqttbrokers.page.refused.noRight)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.mqttbrokers.page.back })).toBeInTheDocument();
    expect(screen.queryByText(en.mqttbrokers.page.refused.missing.replace("{name}", "cameras"))).toBeNull();
    expect(screen.queryByRole("group", { name: "cams" })).toBeNull();
    expect(screen.queryByText(en.mqttbrokers.page.section.live)).toBeNull();
  });

  it("says no broker has the name to a caller who may read brokers", async () => {
    showBroker({ brokerAnswer: notFound });
    expect(await screen.findByText(en.mqttbrokers.page.refused.missing.replace("{name}", "cameras"))).toBeInTheDocument();
    expect(screen.queryByText(en.mqttbrokers.page.refused.noRight)).toBeNull();
  });
});

describe("what a broker measures (T-4043)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/mqttbrokers/cameras`);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("says what the city does next while its custom hostname waits for DNS, and the city's address (T-5874)", async () => {
    const waiting = {
      ...CAMERAS,
      spec: { ...(CAMERAS as { spec: object }).spec, address: "customHostname", hostname: "mqtt.espoo.example" },
      status: { phase: "Pending", conditions: [{ type: "Ready", status: "False", reason: "WaitingForDns", message: "waiting for DNS" }] },
    };
    showBroker({ broker: waiting });
    expect(await screen.findByText(en.mqttbrokers.connect.waitingForDnsTitle)).toBeInTheDocument();
    const alert = document.querySelector("[data-dns='waiting']") as HTMLElement;
    expect(alert.textContent).toContain("Point a CNAME record from mqtt.espoo.example");
    expect(screen.getAllByText("mqtt.espoo.example:8883").length).toBeGreaterThan(0);
  });

  it("shows no DNS notice once the hostname's certificate is issued", async () => {
    const served = { ...CAMERAS, spec: { ...(CAMERAS as { spec: object }).spec, address: "customHostname", hostname: "mqtt.espoo.example" } };
    showBroker({ broker: served });
    expect((await screen.findAllByText("mqtt.espoo.example:8883")).length).toBeGreaterThan(0);
    expect(document.querySelector("[data-dns='waiting']")).toBeNull();
  });

  it("shows each account's connected clients and its roles", async () => {
    showBroker({ measured: () => json(MEASURED) });
    const devices = await screen.findByRole("table", { name: en.mqttbrokers.page.devices });
    const cam1 = within(devices).getByText("cam-1").closest("tr") as HTMLElement;
    expect(within(cam1).getByText("2 connected")).toBeInTheDocument();
    expect(within(cam1).getByText("cams, ops")).toBeInTheDocument();
  });

  it("says not measured, and why, when the broker did not answer the Portal", async () => {
    showBroker({
      measured: () =>
        json({
          measured: false,
          observedAt: "2026-10-12T08:47:00Z",
          messagesPerSecond: null,
          publishers: null,
          reason: "the broker did not answer; try again in a minute",
          accounts: [],
          rates: [],
          refusedPublishes: null,
        }),
    });
    expect(await screen.findByText("the broker did not answer; try again in a minute")).toBeInTheDocument();
    expect(document.querySelector("[data-state='unmeasured']")?.textContent).toBe(en.mqttbrokers.page.state.unmeasured);
    expect(screen.queryByText(en.mqttbrokers.page.figure.rate)).toBeNull();
    expect(screen.queryByRole("meter")).toBeNull();
  });

  it("says not measured, never a zero, when the Portal itself cannot be read", async () => {
    showBroker({ measured: () => new Response("", { status: 502 }) });
    expect(await screen.findByText(en.mqttbrokers.page.notRead, {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.queryByText(en.mqttbrokers.page.figure.rate)).toBeNull();
  });
});
