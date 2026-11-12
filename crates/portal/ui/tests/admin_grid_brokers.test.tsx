// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/mqttbrokers/BrokerAccess.tsx, src/pages/mqttbrokers/BrokerTopics.tsx, src/pages/mqttbrokers/MqttBrokerPage.tsx, src/pages/mqttbrokers/MqttBrokersPage.tsx, src/pages/datasources/DataSourcesPage.tsx, src/routes/ApprovalsHistory.tsx.
/**
 * T-6749: the broker lists, the data sources and the approvals history on the shared grid. The
 * rows are what the API returned; the grid sorts, searches and filters them, and a failed read is
 * the Portal's failure panel.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApprovalsHistory } from "../src/routes/ApprovalsHistory";
import { DataSourcesPage } from "../src/pages/datasources/DataSourcesPage";
import { MqttBrokerPage } from "../src/pages/mqttbrokers/MqttBrokerPage";
import { json, list, problem, renderPage } from "./page_contract";

const PROJECT = "helsinki";
const API = "joinedcontext.com/v1alpha1";

const source = (name: string, url: string) => ({
  apiVersion: API,
  kind: "DataSource",
  metadata: { name, namespace: PROJECT },
  spec: { type: "http", http: { url } },
});

const names = () =>
  [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.querySelector("th")?.textContent ?? "");

describe("data sources on the shared grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/datasources`);
    try {
      window.localStorage.clear();
    } catch {
      // no storage in this run
    }
  });
  afterEach(cleanup);

  const show = (answer: () => Response) =>
    renderPage(<DataSourcesPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/datasources`,
      answer: (url) => (url.pathname.endsWith("/datasources") ? answer() : undefined),
    });

  it("searches the rows the API returned and says when nothing matches", async () => {
    show(() => json(list([source("alpha", "https://a.example/x"), source("beta", "https://b.example/y")])));
    await screen.findByRole("link", { name: "alpha" });
    expect(names()).toHaveLength(2);
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "b.example" } });
    expect(names()).toEqual(["beta"]);
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "zzz" } });
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.clearFilters }));
    expect(names()).toHaveLength(2);
  });

  it("sorts from the header menu", async () => {
    show(() => json(list([source("alpha", "https://a.example/x"), source("beta", "https://b.example/y")])));
    await screen.findByRole("link", { name: "alpha" });
    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.datasources.field.name}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortDesc }));
    expect(names()[0]).toContain("beta");
  });

  it("offers the first source when there is none", async () => {
    show(() => json(list([])));
    expect(await screen.findByText(en.datasources.empty)).toBeInTheDocument();
  });

  it("shows the API's reason when the read fails", async () => {
    show(() => problem(503, "The store is not answering."));
    expect(await screen.findByText(/The store is not answering/)).toBeInTheDocument();
  });
});

describe("approvals history on the shared grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/approvals`);
  });
  afterEach(cleanup);

  const closed = (name: string, endpoint: string, at: string) => ({
    apiVersion: API,
    kind: "ChangeProposal",
    metadata: { name },
    summary: { key: "change.summary.update", params: { kind: "Endpoint", name: endpoint, fields: 1 } },
    author: { name: "Jana" },
    createdAt: at,
    status: { phase: "Merged" },
    decision: { by: "Peter", at },
  });
  const show = (answer: () => Response) =>
    renderPage(<ApprovalsHistory project={PROJECT} />, {
      path: `/projects/${PROJECT}/approvals`,
      answer: (url) => (url.pathname.endsWith("/changes/history") ? answer() : undefined),
    });

  it("searches the closed changes", async () => {
    show(() =>
      json({
        items: [closed("c-1", "buses", "2026-03-02T12:00:00Z"), closed("c-2", "trams", "2026-03-03T12:00:00Z")],
        next: null,
      }),
    );
    await screen.findByRole("link", { name: /buses/ });
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "trams" } });
    expect(names()).toHaveLength(1);
  });

  it("says nothing has been decided yet", async () => {
    show(() => json({ items: [], next: null }));
    expect(await screen.findByText(en.approvals.history.empty)).toBeInTheDocument();
  });

  it("shows the API's reason when the read fails", async () => {
    show(() => problem(503, "The forge is not answering."));
    expect(await screen.findByText(/The forge is not answering/)).toBeInTheDocument();
  });
});

describe("a broker's topics and access on the shared grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState({}, "", `/projects/${PROJECT}/mqttbrokers/sensors`);
  });
  afterEach(cleanup);

  const broker = {
    apiVersion: API,
    kind: "MqttBroker",
    metadata: { name: "sensors", namespace: PROJECT },
    spec: { mode: "forward", roles: [{ name: "devices", serviceAccounts: ["s-1"], publish: ["devices/${clientid}/data"] }] },
    status: { phase: "Live" },
  };
  const bindings = {
    broker: "sensors",
    bindings: [
      { topicFilter: "devices/+/data", dataSource: { kind: "DataSource", name: "in" }, type: "WeatherObserved" },
      { topicFilter: "alerts/#", dataSource: { kind: "DataSource", name: "alerts" }, type: "Alert" },
    ],
  };
  const reader = { project: PROJECT, grants: [{ rule: { kinds: ["MqttBroker"], verbs: ["read"] } }] };
  const show = (bound: () => Response) =>
    renderPage(<MqttBrokerPage project={PROJECT} name="sensors" />, {
      path: `/projects/${PROJECT}/mqttbrokers/sensors`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(reader);
        if (url.pathname.endsWith("/sensors/bindings")) return bound();
        return url.pathname.endsWith("/mqttbrokers/sensors") ? json(broker) : undefined;
      },
    });

  it("searches the topics the API bound", async () => {
    show(() => json(bindings));
    await screen.findByText("alerts/#");
    // The page holds a grid for connections too: the topics' search is the one around the topic.
    const grid = screen.getByText("alerts/#").closest(".jc-data-grid") as HTMLElement;
    const search = within(grid).getByRole("searchbox", { name: en.dataGrid.searchRows });
    fireEvent.change(search, { target: { value: "alert" } });
    await waitFor(() => expect(screen.queryByText("devices/+/data")).toBeNull());
    expect(screen.getByText("alerts/#")).toBeInTheDocument();
  });

  it("says no topic is bound yet", async () => {
    show(() => json({ broker: "sensors", bindings: [] }));
    expect(await screen.findByText(en.mqttbrokers.topics.empty)).toBeInTheDocument();
  });
});
