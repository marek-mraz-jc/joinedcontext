// T-7706: BrokerEditor change handlers: keep switch, capture count and window, limits, port, networks, role name,
// topics, shared reason, and the plain-words list of pending changes that names each.
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { BrokerEditor } from "../src/pages/mqttbrokers/BrokerEditor";
import type { Manifest } from "../src/api/manifest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const BROKER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "MqttBroker",
  metadata: { name: "air", namespace: PROJECT },
  spec: {
    mode: "ingest",
    roles: [{ name: "sensor", publish: ["air/${clientid}/pm10"], serviceAccounts: ["fleet"] }],
  },
} as unknown as Manifest;

const steward = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker", "ServiceAccount"], verbs: ["read", "propose"] } }] };

function show(manifest: Manifest = BROKER) {
  renderPage(<BrokerEditor project={PROJECT} manifest={manifest} />, {
    path: `/projects/${PROJECT}/mqttbrokers/air`,
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(steward);
      if (url.pathname.endsWith("/serviceaccounts")) return json(list([]));
      return undefined;
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

const KEPT = {
  ...(BROKER as unknown as Record<string, unknown>),
  spec: {
    mode: "capture",
    capture: { messagesPerTopic: 20, window: "PT1H" },
    allowedSources: ["10.0.0.0/8"],
    roles: [{ name: "sensor", publish: ["air/${clientid}/pm10"], serviceAccounts: ["fleet"] }],
  },
} as unknown as Manifest;

describe("BrokerEditor change handlers (T-7706)", () => {
  it("says the kept messages changed and which network may no longer connect", async () => {
    show(KEPT);
    const count = await screen.findByLabelText(en.mqttbrokers.field.messagesPerTopic);
    fireEvent.change(count, { target: { value: "30" } });
    expect(await screen.findByText(en.mqttbrokers.edit.change.capture)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.mqttbrokers.edit.remove.replace("{value}", "10.0.0.0/8") }));
    expect(await screen.findByText(en.mqttbrokers.edit.change.sourceRemoved.replace("{source}", "10.0.0.0/8"))).toBeInTheDocument();
  });

  it("names every pending change in words as the person edits capture, limits, port and networks", async () => {
    show();
    const keep = await screen.findByRole("checkbox", { name: en.mqttbrokers.edit.keep });
    await waitFor(() => expect(keep).toBeEnabled());
    fireEvent.click(keep);
    expect(await screen.findByText(en.mqttbrokers.edit.change.mode.replace("{mode}", en.mqttAllowance.mode.capture))).toBeInTheDocument();

    // Capture count and window are written into the draft; an empty count falls back to the default.
    const count = screen.getByLabelText(en.mqttbrokers.field.messagesPerTopic);
    fireEvent.change(count, { target: { value: "50" } });
    expect(count).toHaveValue(50);
    fireEvent.change(count, { target: { value: "" } });
    fireEvent.change(count, { target: { value: "75" } });
    const window = screen.getByLabelText(en.mqttbrokers.field.window) as HTMLSelectElement;
    const other = [...window.options].map((o) => o.value).find((v) => v !== window.value) as string;
    fireEvent.change(window, { target: { value: other } });
    expect(window).toHaveValue(other);

    // Limits and the dedicated port show up as a before and after line each.
    const rate = screen.getByLabelText(en.mqttbrokers.edit.limit.messagesPerSecond);
    fireEvent.change(rate, { target: { value: "5" } });
    const port = screen.getByLabelText(en.mqttbrokers.field.port);
    fireEvent.change(port, { target: { value: "8884" } });
    expect(await screen.findByText(/Dedicated port: 8884/)).toBeInTheDocument();
    expect(screen.getByText(new RegExp(`${en.mqttbrokers.field.messagesPerSecond}: .* → 5`))).toBeInTheDocument();
    fireEvent.change(port, { target: { value: "" } });
    expect(port).toHaveValue(null);

    // A network is added and the change says which.
    const networks = screen.getByRole("textbox", { name: en.mqttbrokers.edit.sources });
    fireEvent.change(networks, { target: { value: "203.0.113.0/24" } });
    fireEvent.keyDown(networks, { key: "Enter" });
    expect(await screen.findByText(en.mqttbrokers.edit.change.sourceAdded.replace("{source}", "203.0.113.0/24"))).toBeInTheDocument();
  });

  it("edits the role: its name, its topics, and the reason when its devices share topics", async () => {
    show();
    const name = await screen.findByLabelText(en.mqttbrokers.field.roleName);
    fireEvent.change(name, { target: { value: " cams " } });
    expect(name).toHaveValue("cams");

    const publish = screen.getByRole("textbox", { name: en.mqttbrokers.acl.publish });
    fireEvent.change(publish, { target: { value: "air/${clientid}/pm25" } });
    fireEvent.keyDown(publish, { key: "Enter" });
    expect(await screen.findByText("air/${clientid}/pm25")).toBeInTheDocument();

    const subscribe = screen.getByRole("textbox", { name: en.mqttbrokers.acl.subscribe });
    fireEvent.change(subscribe, { target: { value: "air/${clientid}/cmd" } });
    fireEvent.keyDown(subscribe, { key: "Enter" });
    expect(await screen.findByText("air/${clientid}/cmd")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: en.mqttbrokers.edit.shared }));
    const reason = await screen.findByLabelText(new RegExp(en.mqttbrokers.edit.reason));
    expect(screen.getAllByText(en.mqttbrokers.edit.reasonMissing).length).toBeGreaterThan(0);
    fireEvent.change(reason, { target: { value: "one cloud id" } });
    expect(reason).toHaveValue("one cloud id");
    await waitFor(() => expect(screen.queryAllByText(en.mqttbrokers.edit.reasonMissing)).toHaveLength(0));
    expect(screen.getByText(en.mqttbrokers.edit.change.roleAdded.replace("{role}", "cams"))).toBeInTheDocument();
  });
});
