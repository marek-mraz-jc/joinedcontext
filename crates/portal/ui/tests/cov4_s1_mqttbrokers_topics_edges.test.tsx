// T-7706: BrokerTopics: the bindings read failing with the server's reason, and a topic search that matches nothing reads every column's text.
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { json, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";

const BROKER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "MqttBroker",
  metadata: { name: "sensors", namespace: PROJECT },
  spec: { mode: "plain", roles: [{ name: "devices", serviceAccounts: ["s-1"], publish: ["devices/${clientid}/data"] }] },
  status: { phase: "Live" },
};

const BOUND = {
  broker: "sensors",
  bindings: [
    {
      topicFilter: "devices/+/data",
      dataSource: { kind: "DataSource", name: "sensors-in" },
      pipeline: { kind: "Pipeline", name: "weather" },
      endpoint: { kind: "Endpoint", name: "weather-write", space: "weather" },
      type: "WeatherObserved",
      attributes: [{ from: "temp", to: "temperature" }, { to: "source" }],
    },
    { dataSource: { kind: "DataSource", hidden: true } },
  ],
};

const reader = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker"], verbs: ["read"] } }] };

function show(bindings: () => Response) {
  renderPage(<MqttBrokerPage project={PROJECT} name="sensors" />, {
    path: `/projects/${PROJECT}/mqttbrokers/sensors`,
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(reader);
      if (url.pathname.endsWith("/sensors/bindings")) return bindings();
      return url.pathname.endsWith("/mqttbrokers/sensors") ? json(BROKER) : undefined;
    },
  });
}

describe("a broker's topics, in their edge cases", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("says why the topics could not be read", async () => {
    show(() => problem(503, "the binding index is rebuilding"));
    expect(await screen.findByText(en.mqttbrokers.topics.errorTitle)).toBeInTheDocument();
    expect(await screen.findByText(/binding index is rebuilding/)).toBeInTheDocument();
  });

  it("finds no topic for a word no column holds", async () => {
    const user = userEvent.setup();
    show(() => json(BOUND));
    await screen.findByText("devices/+/data");
    const topics = screen.getAllByRole("searchbox").at(-1) as HTMLElement;
    await user.type(topics, "zzz-no-such-topic");
    await waitFor(() => expect(screen.queryByText("devices/+/data")).toBeNull());
  });
});
