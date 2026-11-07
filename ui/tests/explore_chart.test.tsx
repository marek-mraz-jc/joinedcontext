// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/explore/ChartFromView.tsx.
// A chart from the explorer (UI-93, T-3257): the attribute's suggestion, and the widget saved to a
// new dashboard or added to an existing one, each proposed after a green check.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { ChartFromView, chartWidget, newDashboard, withWidget } from "../src/pages/explore/ChartFromView";
import { InRouter } from "./pageHarness";

const C = en.explore.chart;
const VIEW = { endpoint: "air-ops", type: "AirQualityObserved", q: "pm10>0" };
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

describe("the widget a chart is saved as", () => {
  it("carries the view's type and filter for a chart over the type, the entity for a history", () => {
    expect(chartWidget("histogram", VIEW, "pm10")).toEqual({ widgetType: "histogram", endpointRef: "air-ops", entityType: "AirQualityObserved", property: "pm10", q: "pm10>0" });
    expect(chartWidget("bar-chart", { ...VIEW, q: " " }, "level")).not.toHaveProperty("q");
    expect(chartWidget("temporal-chart", VIEW, "pm10", "urn:x")).toEqual({ widgetType: "temporal-chart", endpointRef: "air-ops", entityId: "urn:x", property: "pm10" });
  });

  it("goes to the last page with widgets, or on a page of its own, and a new dashboard holds it alone", () => {
    const widget = chartWidget("bar-chart", VIEW, "level");
    const pages = (withWidget(EXISTING, widget, "Charts").spec as { pages: { widgets?: unknown[] }[] }).pages;
    expect(pages[1].widgets).toEqual([{ widgetType: "grid" }, widget]);
    const onlyMap = { ...EXISTING, spec: { ...EXISTING.spec, pages: [{ title: "Map", layers: ["stations"] }] } } as Manifest;
    expect((withWidget(onlyMap, widget, "Charts").spec as { pages: unknown[] }).pages[1]).toEqual({ title: "Charts", layout: "grid-2x2", widgets: [widget] });
    expect(newDashboard("helsinki", "air-charts", "Charts of air", widget, "Charts")).toMatchObject({
      kind: "Dashboard",
      metadata: { name: "air-charts", namespace: "helsinki" },
      spec: { visibility: "project", pages: [{ widgets: [widget] }] },
    });
  });
});

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

describe("charting from the explorer", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("suggests a histogram for a number and saves it to a new dashboard after a green check", async () => {
    const sent: Sent[] = [];
    stub(sent);
    show();
    await userEvent.click(await screen.findByRole("button", { name: C.open }));
    const dialog = await screen.findByRole("dialog");
    const attribute = within(dialog).getByRole("combobox", { name: C.attribute });
    expect(within(attribute).queryByRole("option", { name: "location" })).toBeNull();
    await userEvent.selectOptions(attribute, "pm10");
    expect(await within(dialog).findByRole("radio", { name: C.kinds.histogram.label })).toBeChecked();
    expect(within(dialog).getByText(`Suggested for this attribute: ${C.kinds.histogram.label}.`)).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: C.save }));
    await waitFor(() => expect(sent.map((s) => `${s.method} ${s.search}`)).toEqual(["POST ?dryRun=All", "POST "]));
    expect((sent[1].body as { spec: unknown }).spec).toEqual({
      title: "Charts of AirQualityObserved",
      visibility: "project",
      pages: [{ title: "Charts", layout: "grid-2x2", widgets: [{ widgetType: "histogram", endpointRef: "air-ops", entityType: "AirQualityObserved", property: "pm10", q: "pm10>0" }] }],
    });
    expect(await within(dialog).findByRole("link", { name: /Review it in Approvals/ })).toBeInTheDocument();
  });

  it("adds bars to an existing dashboard by replacing it with the widget appended", async () => {
    const sent: Sent[] = [];
    stub(sent);
    show();
    await userEvent.click(await screen.findByRole("button", { name: C.open }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: C.attribute }), "level");
    expect(await within(dialog).findByRole("radio", { name: C.kinds["bar-chart"].label })).toBeChecked();
    await userEvent.selectOptions(await within(dialog).findByRole("combobox", { name: C.dashboard }), "air");
    await userEvent.click(within(dialog).getByRole("button", { name: C.save }));
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1].method).toBe("PUT");
    expect(sent[1].path).toBe("/api/v1/projects/helsinki/dashboards/air");
    expect((sent[1].body as { spec: { pages: { widgets?: unknown[] }[] } }).spec.pages[1].widgets).toEqual([
      { widgetType: "grid" },
      { widgetType: "bar-chart", endpointRef: "air-ops", entityType: "AirQualityObserved", property: "level", q: "pm10>0" },
    ]);
  });

  it("refuses a new dashboard name the platform would refuse, before anything is sent", async () => {
    const sent: Sent[] = [];
    stub(sent);
    show();
    await userEvent.click(await screen.findByRole("button", { name: C.open }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByRole("combobox", { name: C.attribute }), "pm10");
    const name = await within(dialog).findByRole("textbox", { name: C.name });
    await userEvent.clear(name);
    await userEvent.type(name, "Air_Charts");
    await userEvent.click(within(dialog).getByRole("button", { name: C.save }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(C.badName);
    expect(sent).toEqual([]);
  });
});
