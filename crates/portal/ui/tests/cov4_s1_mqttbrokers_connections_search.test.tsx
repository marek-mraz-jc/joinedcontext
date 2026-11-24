// T-7706: MqttBrokersPage connections grid: searching the edges reads every column's text (to, state).
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { Connections } from "../src/pages/mqttbrokers/MqttBrokersPage";
import type { Manifest } from "../src/api/manifest";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "espoo";
const API = "joinedcontext.com/v1alpha1";
const item = (kind: string, name: string, spec: unknown): Manifest =>
  ({ apiVersion: API, kind, metadata: { name, namespace: PROJECT }, spec }) as Manifest;

const BROKER = item("MqttBroker", "cameras", { roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/frames"] }] });
const WORLD: Record<string, Manifest[]> = {
  datasources: [item("DataSource", "frames", { type: "mqtt", mqtt: { brokerRef: { kind: "MqttBroker", name: "cameras" }, topics: ["cameras/+/frames"] } })],
  pipelines: [item("Pipeline", "camera-frames", { source: { dataSourceRef: "frames" }, targetEndpoint: "urn:ngsi-ld:Endpoint:espoo.fi:espoo:cams-in" })],
  endpoints: [item("Endpoint", "cams-in", { policyRef: "urn:ngsi-ld:Policy:espoo.fi:espoo:cams-write" })],
  policies: [],
  serviceaccounts: [item("ServiceAccount", "cam-1", {})],
};

const READER = {
  project: PROJECT,
  grants: [{ rule: { kinds: ["DataSource", "Pipeline", "Endpoint", "Policy", "ServiceAccount"], verbs: ["read"] } }],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the connections of a broker, searched", () => {
  it("keeps the edges whose destination holds the word and drops the rest", async () => {
    const user = userEvent.setup();
    renderPage(<Connections project={PROJECT} brokers={[BROKER]} />, {
      path: `/projects/${PROJECT}/mqttbrokers`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(READER);
        const plural = url.pathname.split("/").pop() ?? "";
        return WORLD[plural] ? json(list(WORLD[plural])) : undefined;
      },
    });
    await screen.findByRole("table");
    await waitFor(() => expect(within(screen.getByRole("table")).getAllByRole("row").length).toBeGreaterThan(2));
    const before = within(screen.getByRole("table")).getAllByRole("row").length;

    const search = screen.getByRole("searchbox");
    await user.type(search, "cams-in");
    const after = within(screen.getByRole("table")).getAllByRole("row").length;
    expect(after).toBeLessThan(before);
    expect(after).toBeGreaterThan(1);

    // A word no row holds in any column: nothing is left but the empty notice.
    await user.clear(search);
    await user.type(search, "zzz-nothing");
    expect(screen.queryByText("cams-in")).toBeNull();
  });
});
