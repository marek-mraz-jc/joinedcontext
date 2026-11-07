// T-3218: the explorer as a new user meets it. The type list says which types the chosen
// endpoint lets them read, so nobody picks one only to meet an empty grid, and two endpoints of
// one title are told apart by their names.
import en from "../src/locales/en.json";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { queryKeys } from "../src/api/client";
import { ExplorePage } from "../src/pages/explore/ExplorePage";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, title: string, slug: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki", title: { en: title } },
  spec: { contextSpaceRef: "helsinki", slug, audience: "organization", enabledRepresentations: ["ngsi-ld"] },
});
const ENDPOINTS = list([
  endpoint("helsinki-bikes", "Helsinki city bike stations", "scsd2eehkx42n53z2zyd6vshfh7s7irf"),
  endpoint("helsinki-bikes-ops", "Helsinki city bike stations", "abcd2eehkx42n53z2zyd6vshfh7s7irf"),
  endpoint("helsinki-events", "Helsinki events", "efgh2eehkx42n53z2zyd6vshfh7s7irf"),
]);
const SPACES = list([
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: "helsinki-city" } },
]);
const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-city", namespace: "helsinki" },
    spec: { classes: ["BikeHireDockingStation", "Event", "WeatherObserved"], linkml: "models/helsinki-city.yaml" },
  },
]);

function open(access: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = typeof input === "string" ? input : ((input as Request).url ?? "");
      if (url.includes("/access")) {
        return Promise.resolve(new Response(JSON.stringify(access), { status: 200, headers: { "Content-Type": "application/json" } }));
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer for a new user (T-3218)", () => {
  it("offers the types the endpoint grants first, and groups the rest as not readable", async () => {
    open({ permissions: [{ resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity"] }] });
    const select = await screen.findByLabelText(/Entity type/i);
    await waitFor(() =>
      expect([...select.querySelectorAll("option")].map((o) => o.textContent)).toEqual(["—", "WeatherObserved", "BikeHireDockingStation", "Event"]),
    );
    // The ungranted ones stand apart, in a group that says why (T-5622).
    const group = select.querySelector("optgroup");
    expect(group?.getAttribute("label")).toBe("Not readable through this endpoint");
    expect([...(group?.querySelectorAll("option") ?? [])].map((o) => o.textContent)).toEqual(["BikeHireDockingStation", "Event"]);
    // The values stay the type names: a pick is the same type whatever the label says.
    expect([...select.querySelectorAll("option")].map((o) => o.getAttribute("value"))).toEqual(["", "WeatherObserved", "BikeHireDockingStation", "Event"]);
  });

  it("keeps one plain list when a grant covers every type", async () => {
    open({ permissions: [{ actions: ["queryEntity"] }] });
    const select = await screen.findByLabelText(/Entity type/i);
    await waitFor(() => expect(select.querySelectorAll("option")).toHaveLength(4));
    expect([...select.querySelectorAll("option")].map((o) => o.textContent)).toEqual(["—", "BikeHireDockingStation", "Event", "WeatherObserved"]);
    expect(select.querySelector("optgroup")).toBeNull();
  });

  it("tells two endpoints of one title apart by their names, and leaves a unique title alone", async () => {
    open({ permissions: [] });
    const endpoints = await screen.findByLabelText(en.explore.endpoint);
    const labels = [...endpoints.querySelectorAll("option")].map((o) => o.textContent);
    expect(labels).toEqual([
      "—",
      "Helsinki city bike stations (helsinki-bikes)",
      "Helsinki city bike stations (helsinki-bikes-ops)",
      "Helsinki events",
    ]);
  });
});
