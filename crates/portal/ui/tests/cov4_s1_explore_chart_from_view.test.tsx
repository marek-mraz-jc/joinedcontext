// T-7706: ChartFromView gaps: a history chart of one picked entity, and closing the dialog drops the saved change.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { ChartFromView } from "../src/pages/explore/ChartFromView";
import { InRouter } from "./pageHarness";

const C = en.explore.chart;
const ROWS = [
  { id: "urn:ngsi-ld:AirQualityObserved:1", type: "AirQualityObserved", pm10: { type: "Property", value: 12 }, level: { type: "Property", value: "good" } },
  { id: "urn:ngsi-ld:AirQualityObserved:2", type: "AirQualityObserved", pm10: { type: "Property", value: 40 }, level: { type: "Property", value: "bad" } },
];
const EXISTING: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "air", namespace: "helsinki" },
  spec: { title: "Air", visibility: "project", pages: [{ title: "Map", layers: ["stations"] }, { title: "Charts", widgets: [{ widgetType: "grid" }] }] },
} as Manifest;

interface Sent {
  method: string;
  path: string;
  search: string;
  body: unknown;
}

function stub(sent: Sent[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin));
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.includes("/entities")) return json(ROWS);
      if (url.pathname === "/api/v1/projects/helsinki/dashboards" && request.method === "GET") {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [EXISTING] });
      }
      if (url.pathname.startsWith("/api/v1/projects/helsinki/dashboards")) {
        sent.push({ method: request.method, path: url.pathname, search: url.search, body: JSON.parse(await request.text()) });
        return url.search.includes("dryRun")
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-0000077", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
}

function show() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ChartFromView
            project="helsinki"
            endpoint="air-ops"
            slug="air-slug"
            type="AirQualityObserved"
            q="pm10>0"
            slots={[
              { name: "pm10", range: "float", kind: "Property" },
              { name: "level", range: "string", kind: "Property", values: ["good", "bad"] },
              { name: "location", kind: "GeoProperty" },
            ]}
          />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("a history chart and a closed dialog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("saves the history of the entity the person picked", async () => {
    const sent: Sent[] = [];
    stub(sent);
    show();
    await userEvent.click(await screen.findByRole("button", { name: C.open }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: C.attribute }), "pm10");
    await userEvent.click(await within(dialog).findByRole("radio", { name: C.kinds["temporal-chart"].label }));
    const entity = await within(dialog).findByRole("combobox", { name: C.entity });
    await userEvent.selectOptions(entity, "urn:ngsi-ld:AirQualityObserved:2");
    await userEvent.click(within(dialog).getByRole("button", { name: C.save }));
    await waitFor(() => expect(sent).toHaveLength(2));
    const widgets = (sent[1].body as { spec: { pages: { widgets: unknown[] }[] } }).spec.pages[0].widgets;
    expect(widgets).toEqual([
      { widgetType: "temporal-chart", endpointRef: "air-ops", entityId: "urn:ngsi-ld:AirQualityObserved:2", property: "pm10" },
    ]);
  });

  it("drops the proposed change when the dialog is closed, and opens fresh the next time", async () => {
    const sent: Sent[] = [];
    stub(sent);
    show();
    await userEvent.click(await screen.findByRole("button", { name: C.open }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: C.attribute }), "pm10");
    await userEvent.click(within(dialog).getByRole("button", { name: C.save }));
    expect(await within(dialog).findByRole("link", { name: /Review it in Approvals/ })).toBeInTheDocument();
    await userEvent.click(within(dialog).getAllByRole("button", { name: en.app.close })[0]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await userEvent.click(screen.getByRole("button", { name: C.open }));
    const again = await screen.findByRole("dialog");
    expect(within(again).queryByRole("link", { name: /Review it in Approvals/ })).toBeNull();
    expect(within(again).getByRole("button", { name: C.save })).toBeInTheDocument();
  });
});
