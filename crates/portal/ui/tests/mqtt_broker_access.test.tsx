/**
 * T-5876 (ADR-N-048 §2.13): a broker card's "who can do what" panel asks the explain route for
 * one ServiceAccount the person reads and shows the roles it holds now, none, a grant out of its
 * window, or the refusal; and the ServiceAccount form offers the brokers' roles as grants.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { BrokerAccess } from "../src/pages/mqttbrokers/BrokerAccess";
import { mqttRoleChoices } from "../src/pages/access/ServiceAccounts";
import type { Manifest } from "../src/api/manifest";
import { json, list, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const BROKER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "MqttBroker",
  metadata: { name: "air", namespace: PROJECT },
  spec: { roles: [{ name: "sensor", publish: ["air/${clientid}/pm10"] }] },
  status: { phase: "Live" },
};

const account = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name, namespace: PROJECT },
  spec: { roles: [] },
});

const reader = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read"] } }] };

const HELD = {
  broker: "air",
  account: "fleet",
  at: "2026-10-29T12:00:00Z",
  roles: [{ role: "sensor", publish: ["air/${clientid}/pm10"], subscribe: [], legacy: true, bound: true, until: "2026-12-01T00:00:00Z" }],
  outOfWindow: [],
};

function show(options: { accounts?: string[]; access?: (account: string) => Response }) {
  const asked: string[] = [];
  renderPage(<MqttBrokerPage project={PROJECT} name="air" />, {
    path: `/projects/${PROJECT}/mqttbrokers/air`,
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(reader);
      if (url.pathname.endsWith("/air/access")) {
        const who = url.searchParams.get("account") ?? "";
        asked.push(who);
        return (options.access ?? (() => json(HELD)))(who);
      }
      if (url.pathname.endsWith("/serviceaccounts")) return json(list((options.accounts ?? ["fleet"]).map(account)));
      if (url.pathname.endsWith("/air/bindings")) return json({ broker: "air", bindings: [] });
      return url.pathname.endsWith("/mqttbrokers/air") ? json(BROKER) : undefined;
    },
  });
  return asked;
}

async function panel(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Who can do what" });
}

describe("who can do what on a broker", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(cleanup);

  it("shows the roles the account holds with their topics, grant and end", async () => {
    const asked = show({});
    const region = await panel();
    const table = await within(region).findByRole("table", { name: "Roles of air that fleet holds now" });
    expect(within(table).getByText("sensor")).toBeTruthy();
    expect(within(table).getByText("air/${clientid}/pm10")).toBeTruthy();
    expect(within(table).getByText("Role binding")).toBeTruthy();
    expect(within(table).getByText("Listed on the role")).toBeTruthy();
    expect(within(region).getByText(/the older way/)).toBeTruthy();
    expect(asked).toEqual(["fleet"]);
  });

  it("asks again for the account picked and says when it holds nothing", async () => {
    const asked = show({
      accounts: ["fleet", "spare"],
      access: (who) => json(who === "spare" ? { ...HELD, account: "spare", roles: [] } : HELD),
    });
    const region = await panel();
    await within(region).findByRole("table");
    fireEvent.change(within(region).getByLabelText(/Service account/), { target: { value: "spare" } });
    expect(await within(region).findByText("spare holds no role of this broker")).toBeTruthy();
    expect(within(region).getByText(/refused when they connect/)).toBeTruthy();
    expect(asked).toEqual(["fleet", "spare"]);
  });

  it("lists a grant past its end", async () => {
    show({ access: () => json({ ...HELD, roles: [], outOfWindow: [{ grant: "mqtt/*/sensor", notAfter: "2026-10-01T00:00:00Z" }] }) });
    const region = await panel();
    expect(await within(region).findByText("Grants of this broker that are not in force")).toBeTruthy();
    expect(within(region).getByText("mqtt/*/sensor")).toBeTruthy();
    expect(within(region).getByText(/^ended /)).toBeTruthy();
  });

  it("says why when the route refuses", async () => {
    renderPage(<BrokerAccess project={PROJECT} broker="air" />, {
      path: `/projects/${PROJECT}/mqttbrokers`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(reader);
        if (url.pathname.endsWith("/serviceaccounts")) return json(list([account("fleet")]));
        if (url.pathname.endsWith("/air/access")) return problem(403, "you may not read the MQTT brokers of project 'helsinki'");
        return undefined;
      },
    });
    const region = await panel();
    expect(await within(region).findByText("You may not read this project's MQTT brokers")).toBeTruthy();
  });

  it("tells a project without accounts what to do next and asks nothing", async () => {
    const asked = show({ accounts: [] });
    const region = await panel();
    await waitFor(() => expect(within(region).getByText("No service account to explain")).toBeTruthy());
    expect(asked).toEqual([]);
  });
});

describe("the brokers' roles as account grants", () => {
  it("offers each declared role on its broker and on every broker", () => {
    const t = ((key: string, values?: Record<string, string>) => `${key} ${JSON.stringify(values)}`) as never;
    const choices = mqttRoleChoices([BROKER as unknown as Manifest], t).map(([name]) => name);
    expect(choices).toEqual(["mqtt/air/sensor", "mqtt/*/sensor"]);
    expect(mqttRoleChoices([{ ...BROKER, spec: {} } as unknown as Manifest], t)).toEqual([]);
  });
});
