/**
 * T-5533: a broker card's topics (which topic fills which attribute, API/01 §47) and, for a
 * person who may approve brokers, the messages a capture broker kept. Nobody else gets the
 * panel, and the Portal never asks the capture route for them.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { Attributes, CapturedList, readablePayload, topicMatches } from "../src/pages/mqttbrokers/BrokerTopics";
import { json, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const broker = (mode: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "MqttBroker",
  metadata: { name: "sensors", namespace: PROJECT },
  spec: { mode, capture: { messagesPerTopic: 20, window: "PT1H" }, roles: [{ name: "devices", serviceAccounts: ["s-1"], publish: ["devices/${clientid}/data"] }] },
  status: { phase: "Live" },
});

const BOUND = {
  broker: "sensors",
  bindings: [
    {
      topicFilter: "devices/+/data",
      dataSource: { kind: "DataSource", name: "sensors-in" },
      pipeline: { kind: "Pipeline", name: "weather" },
      endpoint: { kind: "Endpoint", name: "weather-write", space: "weather" },
      type: "WeatherObserved",
      attributes: [
        { from: "temp", to: "temperature" },
        { from: "lon, lat", to: "location" },
        { to: "source" },
      ],
    },
    { dataSource: { kind: "DataSource", hidden: true } },
  ],
};

const CAPTURED = {
  broker: "sensors",
  mode: "capture",
  window: "PT1H",
  messagesPerTopic: 20,
  truncated: false,
  messages: [
    { topic: "devices/s-1/data", receivedAt: "2026-10-24T20:41:02Z", clientId: "helsinki-s-1", bytes: 26, payload: '{"temp":21.5,"x":"<b>x</b>"}', encoding: "utf-8" },
    { topic: "devices/s-2/data", receivedAt: "2026-10-24T20:40:00Z", bytes: 3, payload: "/wAB", encoding: "base64" },
  ],
};

const approver = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker"], verbs: ["read", "approve"] } }] };
const reader = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker"], verbs: ["read"] } }] };

function show(options: { mode?: string; permissions?: unknown; bindings?: () => Response; captured?: () => Response }) {
  const calls: string[] = [];
  renderPage(<MqttBrokerPage project={PROJECT} name="sensors" />, {
    path: `/projects/${PROJECT}/mqttbrokers/sensors`,
    answer: (url, request) => {
      calls.push(`${request.method} ${url.pathname}`);
      if (url.pathname.endsWith("/permissions/me")) return json(options.permissions ?? reader);
      if (url.pathname.endsWith("/sensors/bindings")) return (options.bindings ?? (() => json(BOUND)))();
      if (url.pathname.endsWith("/sensors/captured")) {
        if (request.method === "DELETE") return new Response(null, { status: 204 });
        return (options.captured ?? (() => json(CAPTURED)))();
      }
      return url.pathname.endsWith("/mqttbrokers/sensors") ? json(broker(options.mode ?? "capture")) : undefined;
    },
  });
  return calls;
}

describe("a broker's topics and captured messages", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("matches MQTT filters level by level", () => {
    expect(topicMatches("devices/+/data", "devices/s-1/data")).toBe(true);
    expect(topicMatches("devices/+/data", "devices/s-1/other")).toBe(false);
    expect(topicMatches("devices/#", "devices/a/b/c")).toBe(true);
    expect(topicMatches("devices/+", "devices/a/b")).toBe(false);
    expect(readablePayload({ payload: '{"a":1}', encoding: "utf-8" })).toBe('{\n  "a": 1\n}');
    expect(readablePayload({ payload: "not json", encoding: "utf-8" })).toBe("not json");
  });

  it("shows each topic with its type, attribute chips and Endpoint, and a hidden hop without its name", async () => {
    show({});
    const filter = await screen.findByText("devices/+/data");
    const row = filter.closest("tr") as HTMLElement;
    expect(row.closest("table")?.querySelector("caption")?.textContent).toBe("Which topic of sensors fills which attribute");
    expect(within(row).getByText("WeatherObserved")).toBeInTheDocument();
    expect(within(row).getByText("temp → temperature")).toBeInTheDocument();
    expect(within(row).getByText("lon, lat → location")).toBeInTheDocument();
    expect(within(row).getByText("source (computed)")).toBeInTheDocument();
    expect(within(row).getByText("weather-write").closest("a")).toHaveAttribute("href", `/projects/${PROJECT}/endpoints/weather-write`);
    expect(screen.getByText("A DataSource you may not read")).toBeInTheDocument();
  });

  it("says what to do next when no topic is bound", async () => {
    show({ bindings: () => json({ broker: "sensors", bindings: [] }) });
    expect(await screen.findByText(en.mqttbrokers.topics.empty)).toBeInTheDocument();
    expect(screen.getByText(/picks sensors as its platform broker/)).toBeInTheDocument();
  });

  it("never renders the capture panel nor calls its route for a person who may not approve brokers", async () => {
    const calls = show({ permissions: reader });
    await screen.findByText("devices/+/data");
    expect(screen.queryByText(en.mqttbrokers.captured.title)).toBeNull();
    expect(calls.some((call) => call.endsWith("/captured"))).toBe(false);
    expect(screen.queryByText(en.mqttbrokers.topics.last)).toBeNull();
  });

  it("shows an approver the kept messages newest first, as text, with the last message per topic", async () => {
    show({ permissions: approver });
    const payload = await screen.findByLabelText("Payload of devices/s-1/data");
    expect(payload.textContent).toBe('{\n  "temp": 21.5,\n  "x": "<b>x</b>"\n}');
    expect(payload.querySelector("b")).toBeNull();
    const kept = screen.getByRole("list", { name: en.mqttbrokers.captured.title });
    expect(within(kept).getByText("helsinki-s-1")).toBeInTheDocument();
    expect(within(kept).getByText("base64")).toBeInTheDocument();
    const row = screen.getByText("devices/+/data").closest("tr") as HTMLElement;
    expect(within(row).getAllByRole("cell").at(-1)?.textContent).not.toBe("—");
  });

  it("deletes the capture only after the confirm", async () => {
    const calls = show({ permissions: approver });
    fireEvent.click(await screen.findByRole("button", { name: en.mqttbrokers.captured.clear }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Delete every captured message of sensors?")).toBeInTheDocument();
    expect(calls.some((call) => call.startsWith("DELETE"))).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: en.mqttbrokers.captured.clear }));
    await waitFor(() => expect(calls).toContain(`DELETE /api/v1/projects/${PROJECT}/mqttbrokers/sensors/captured`));
  });

  it("says when nothing is kept yet, and when the broker keeps nothing at all", async () => {
    show({ permissions: approver, captured: () => json({ ...CAPTURED, messages: [] }) });
    expect(await screen.findByText(en.mqttbrokers.captured.empty)).toBeInTheDocument();
    cleanup();
    const calls = show({ permissions: approver, mode: "ingest" });
    expect(await screen.findByText(/sensors does not keep messages/)).toBeInTheDocument();
    expect(calls.some((call) => call.endsWith("/captured"))).toBe(false);
  });

  it("says why when the capture cannot be read", async () => {
    show({ permissions: approver, captured: () => problem(503, "the capture store did not answer; try again in a minute") });
    expect(await screen.findByText(en.mqttbrokers.captured.errorTitle, {}, { timeout: 5000 })).toBeInTheDocument();
  });
});

describe("the topic and capture parts alone", () => {
  it("names each attribute a topic fills, and says when the pipeline maps in code", async () => {
    renderPage(<Attributes row={BOUND.bindings[0]} />, { answer: () => undefined });
    const list = await screen.findByRole("list", { name: en.mqttbrokers.topics.attributes });
    expect(within(list).getByText("temp → temperature")).toBeInTheDocument();
    expect(within(list).getByText("source (computed)")).toBeInTheDocument();
    cleanup();
    renderPage(<Attributes row={{ ...BOUND.bindings[0], attributes: undefined }} />, { answer: () => undefined });
    expect(await screen.findByText(en.mqttbrokers.topics.byHand)).toBeInTheDocument();
  });

  it("lists kept messages and asks before it deletes them", async () => {
    const calls: string[] = [];
    renderPage(<CapturedList project={PROJECT} broker="sensors" captured={CAPTURED} heading={<h3>kept</h3>} />, {
      answer: (url, request) => {
        calls.push(`${request.method} ${url.pathname}`);
        return request.method === "DELETE" ? new Response(null, { status: 204 }) : undefined;
      },
    });
    expect((await screen.findByRole("list", { name: en.mqttbrokers.captured.title })).children).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: en.mqttbrokers.captured.clear }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: en.mqttbrokers.captured.clear }));
    await waitFor(() => expect(calls).toContain(`DELETE /api/v1/projects/${PROJECT}/mqttbrokers/sensors/captured`));
  });
});
