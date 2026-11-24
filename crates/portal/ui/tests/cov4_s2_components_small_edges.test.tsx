// T-7706: one missed branch each in the CKAN page's form checks and activity feed, the attribute picker's offer
// rule, the geomap markers, the data agreement's bare offer reference, and a Policy with nobody and no operation.
import { cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { faults, lastRunByEndpoint } from "../src/pages/ckan/CkanPage";
import { offered } from "../src/components/pickers/AttributePicker";
import { markersOf } from "../src/components/panels/GeomapPanel";
import type { Frame } from "../src/components/panels/frames";
import { fromDataAgreementManifest } from "../src/schemas/dataagreement";
import { PolicyGrants } from "../src/components/endpoints/PolicyGrants";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("CKAN instance form checks", () => {
  it("refuses a secret key with characters a Secret key cannot hold", () => {
    const draft = { name: "open-data", url: "https://ckan.example", organizationDefault: "", secretName: "ckan-token", secretKey: "bad key!" };
    expect(faults(draft)).toEqual({ secretKey: "ckan.instances.secretKeyInvalid" });
    expect(faults({ ...draft, secretKey: "token" })).toEqual({});
  });
});

describe("lastRunByEndpoint", () => {
  it("skips events of other objects and keeps the newest event of each endpoint", () => {
    const events = [
      { time: "2026-10-02T00:00:00Z", details: { object: "endpoints/air" } },
      { time: "2026-10-01T00:00:00Z", details: { object: "endpoints/air" } },
      { time: "2026-10-03T00:00:00Z", details: { object: "pipelines/air" } },
      { time: "2026-10-03T00:00:00Z", details: null },
      { time: "2026-10-03T00:00:00Z", details: { object: 7 } },
    ] as unknown as Parameters<typeof lastRunByEndpoint>[0];
    expect(Object.keys(lastRunByEndpoint(events))).toEqual(["air"]);
    expect(lastRunByEndpoint(events).air.time).toBe("2026-10-02T00:00:00Z");
  });
});

describe("AttributePicker offered", () => {
  it("leaves out computed slots and slots of another kind", () => {
    expect(offered({ name: "share", kind: "Property", formula: "{a}" } as never, "any", false)).toBe(false);
    expect(offered({ name: "ref", kind: "Relationship" }, "Property", false)).toBe(false);
    expect(offered({ name: "n", kind: "Property", range: "integer" }, "Property", true)).toBe(true);
    expect(offered({ name: "t", kind: "Property", range: "string" }, "Property", true)).toBe(false);
    expect(offered({ name: "r", kind: "Relationship" }, "any", false)).toBe(true);
  });
});

describe("geomap markers", () => {
  const frames: Frame[] = [
    {
      fields: [
        { name: "lat", type: "number", values: [60.1] },
        { name: "lon", type: "number", values: [24.9] },
      ],
    },
  ];
  it("reads the coordinate fields a layer names and notes a second layer and a view that is not fit", () => {
    const markers = markersOf(frames, {
      layers: [{ type: "markers", location: { mode: "coords", latitude: "lat", longitude: "lon" } }, { type: "heatmap" }],
      view: { id: "coords" },
    });
    expect(markers.features.features.map((feature) => feature.geometry.coordinates)).toEqual([[24.9, 60.1]]);
    expect(markers.notes.map((note) => note.key)).toEqual(expect.arrayContaining(["view", "layers"]));
  });
});

describe("data agreement offer reference", () => {
  it("reads an offer written as a bare name", () => {
    expect(fromDataAgreementManifest({ metadata: { name: "a" }, spec: { offerRef: "bikes-offer" } }).offerRef).toBe("bikes-offer");
    expect(fromDataAgreementManifest({ metadata: { name: "a" }, spec: { offerRef: { kind: "Offer", name: "typed" } } }).offerRef).toBe("typed");
    expect(fromDataAgreementManifest({ metadata: { name: "a" }, spec: {} }).offerRef).toBeUndefined();
  });
});

describe("a Policy that names nobody and no operation", () => {
  it("says so in words instead of an empty grant", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ kind: "List", items: [] }), { headers: { "Content-Type": "application/json" } })));
    const policy = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: "empty-grant", namespace: "helsinki", title: { en: "An empty grant" } },
      spec: { contextSpaceRef: { kind: "ContextSpace", name: "helsinki" }, assignee: { kind: "user", id: "" }, operations: [] },
    } as unknown as Manifest;
    const root = createRootRoute({ component: () => <PolicyGrants project="helsinki" binding={{ kind: "all", policies: [policy] }} pending={false} /> });
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <RouterProvider router={createRouter({ routeTree: root })} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText(en.endpoints.page.policyEmpty)).toBeInTheDocument();
    expect(screen.getByText(en.policies.sentence.nobody)).toBeInTheDocument();
  });
});
