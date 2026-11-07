/**
 * T-3839: the MQTT form picks a broker of the project (PL-76, MF-54). The brokers are listed with
 * their state, a picked one replaces the URLs and the credential, and a project with none, or a
 * caller who may not read them, keeps the external broker alone.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DataSourcesPage, toEnvelope } from "../src/pages/datasources/DataSourcesPage";
import { brokerOptions, dataSourceSchema } from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "espoo";
const t = (key: string) => key;

function broker(name: string, phase: string | undefined, publish: string[]) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "MqttBroker",
    metadata: { name, namespace: PROJECT },
    spec: { roles: [{ name: "cameras", serviceAccounts: ["cam-1"], publish }] },
    ...(phase ? { status: { phase } } : {}),
  };
}

function mqttOf(schema: JsonSchema): JsonSchema {
  return (schema.properties as Record<string, JsonSchema>).mqtt;
}

describe("the broker picker's schema", () => {
  it("offers no picker and asks for URLs when the project has no broker", () => {
    const mqtt = mqttOf(dataSourceSchema(t, "mqtt", [], []));
    expect((mqtt.properties as Record<string, unknown>).brokerRef).toBeUndefined();
    expect(mqtt.required).toEqual(["urls", "topics"]);
  });

  it("lists every broker with its state, a down one included, and the external choice first", () => {
    const brokers = brokerOptions([
      broker("cameras", "Live", ["cameras/${clientid}/frames"]),
      broker("air", "Error", ["air/+/reading"]),
      broker("new", undefined, ["new/#"]),
    ]);
    const mqtt = mqttOf(dataSourceSchema(t, "mqtt", [], brokers));
    const name = ((mqtt.properties as Record<string, JsonSchema>).brokerRef.properties as Record<string, JsonSchema>)
      .name;
    expect(name.oneOf).toEqual([
      { const: "", title: "datasources.field.externalBroker" },
      { const: "air", title: "air · phase.error" },
      { const: "cameras", title: "cameras · phase.live" },
      { const: "new", title: "new · phase.notDeployed" },
    ]);
    expect(mqtt.required).toEqual(["topics"]);
    const topics = (mqtt.properties as Record<string, JsonSchema>).topics.items as JsonSchema;
    expect(topics.examples).toEqual(["air/+/reading", "cameras/${clientid}/frames", "new/#"]);
  });

  it("reads a broker with no roles or a malformed one as publishing nothing", () => {
    expect(brokerOptions([{ metadata: { name: "bare" }, spec: { roles: [{ publish: "x" }] } }])).toEqual([
      { name: "bare", publish: [] },
    ]);
  });
});

describe("the manifest a picked broker writes", () => {
  it("names the broker and drops the URLs and the credential it replaces", () => {
    const envelope = toEnvelope(PROJECT, "mqtt", {
      name: "frames",
      mqtt: {
        brokerRef: { name: "cameras" },
        urls: ["tls://old:8883"],
        username: "collector",
        passwordRef: { name: "mqtt-secret", key: "password" },
        topics: ["cameras/+/frames"],
      },
    });
    expect(envelope.spec).toEqual({
      type: "mqtt",
      mqtt: { brokerRef: { kind: "MqttBroker", name: "cameras" }, topics: ["cameras/+/frames"] },
    });
  });

  it("keeps an external broker as it was and writes no brokerRef for the empty choice", () => {
    const envelope = toEnvelope(PROJECT, "mqtt", {
      name: "city",
      mqtt: {
        brokerRef: { name: "" },
        urls: ["tls://mqtt.espoo.fi:8883"],
        passwordRef: { name: "mqtt-secret", key: "password" },
        topics: ["sensors/#"],
      },
    });
    expect(envelope.spec).toEqual({
      type: "mqtt",
      mqtt: {
        urls: ["tls://mqtt.espoo.fi:8883"],
        passwordRef: { name: "mqtt-secret", key: "password" },
        topics: ["sensors/#"],
      },
    });
  });
});

describe("the MQTT form on the page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/datasources`);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  async function openMqtt() {
    await userEvent.click((await screen.findAllByRole("button", { name: en.datasources.add }))[0]);
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.datasources.field.type), "mqtt");
    return dialog;
  }

  it("picks a broker of the project by name and state", async () => {
    renderPage(<DataSourcesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/datasources`,
      answer: (url) =>
        url.pathname.endsWith("/mqttbrokers") ? json(list([broker("cameras", "Live", ["cameras/#"])])) : undefined,
    });
    const dialog = await openMqtt();
    const picker = await within(dialog).findByLabelText(en.datasources.field.broker);
    expect(within(picker).getByRole("option", { name: "cameras · Live" })).toBeInTheDocument();
    expect(within(picker).getByRole("option", { name: en.datasources.field.externalBroker })).toBeInTheDocument();
  });

  it("never asks for the brokers of a caller who may not read them, and keeps the external broker", async () => {
    const asked: string[] = [];
    renderPage(<DataSourcesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/datasources`,
      answer: (url) => {
        asked.push(url.pathname);
        return url.pathname.endsWith("/permissions/me")
          ? json({ grants: [{ rule: { kinds: ["DataSource"], verbs: ["read", "propose"] } }] })
          : undefined;
      },
    });
    const dialog = await openMqtt();
    expect(within(dialog).getByLabelText(/Broker URLs/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(en.datasources.field.broker)).not.toBeInTheDocument();
    await waitFor(() => expect(asked.some((path) => path.endsWith("/permissions/me"))).toBe(true));
    expect(asked.some((path) => path.endsWith("/mqttbrokers"))).toBe(false);
  });
});
