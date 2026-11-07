/**
 * T-0792: a dashboard is pages, not one page. The reader chooses which one, and an analytics
 * page draws its widgets rather than an empty map (UI-17, UI-18, UI-19).
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { pointsOf, polylineOf } from "../src/components/dashboards/TemporalChart";

vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource() {}
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
      }
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    off() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 24.9, getSouth: () => 60.1, getEast: () => 25, getNorth: () => 60.2 };
    }
  }
  const Popup = class {
    setLngLat() {
      return this;
    }
    setHTML() {
      return this;
    }
    addTo() {
      return this;
    }
  };
  return { Map: FakeMap, setWorkerUrl: () => undefined, NavigationControl: class {}, Popup, default: { Map: FakeMap, NavigationControl: class {}, Popup } };
});

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "helsinki-air", namespace: "helsinki" },
  spec: { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
};
const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "stations", namespace: "helsinki" },
  spec: { sourceEndpointRef: "helsinki-air", entityType: "AirQualityObserved", style: "circle" },
};
const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "air", namespace: "helsinki" },
  spec: {
    title: { en: "Air" },
    visibility: "project",
    pages: [
      { title: "Map", layout: "full-map", layers: ["stations"] },
      {
        title: "Analyses",
        layout: "grid-2x2",
        widgets: [
          {
            widgetType: "temporal-chart",
            endpointRef: "helsinki-air",
            entityId: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi",
            property: "pm10",
          },
        ],
      },
    ],
  },
};
const HISTORY = {
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kamppi",
  type: "AirQualityObserved",
  pm10: { type: "Property", values: [[12, "2026-09-15T08:00:00Z"], [18, "2026-09-15T09:00:00Z"]] },
};

function renderDashboards() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? new URL(input, "http://localhost") : new URL((input as Request).url);
    const json = (body: unknown) =>
      Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.includes("/temporal/entities/")) {
      return json(HISTORY);
    }
    if (url.pathname.includes("/api/endpoint/")) {
      return json({ type: "FeatureCollection", features: [] });
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/dashboards")) {
      return json(list([DASHBOARD]));
    }
    if (url.pathname.endsWith("/layers")) {
      return json(list([LAYER]));
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json(list([ENDPOINT]));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("a dashboard of two pages", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/dashboards");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("names both pages and draws the second one's widget", async () => {
    const fetchMock = renderDashboards();

    const tabs = await screen.findByRole("tablist", { name: en.dashboards.pages });
    expect(await screen.findByRole("tab", { name: "Map" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("tab", { name: "Analyses" })).toBeInTheDocument();
    expect(tabs).toBeInTheDocument();
    // The first page's layer is drawn, and its features are read.
    expect(screen.getByLabelText(`${en.dashboards.show}: stations`)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Analyses" }));

    // The analytics page is the widget's, not the first page's map legend.
    expect(await screen.findByText(/pm10 over time/i)).toBeInTheDocument();
    expect(screen.queryByLabelText(`${en.dashboards.show}: stations`)).not.toBeInTheDocument();
    await waitFor(() => {
      expect(
        fetchMock.mock.calls.some((call) => {
          const target = call[0] as Request | string;
          const href = typeof target === "string" ? target : target.url;
          return href.includes("/temporal/entities/") && href.includes("attrs=pm10");
        }),
      ).toBe(true);
    });
    expect(await screen.findByRole("img", { name: /pm10 .* over time/i })).toBeInTheDocument();
  });
});

describe("the readings of a temporal widget", () => {
  it("takes either shape the temporal representation answers with, in time order", () => {
    const pairs = pointsOf({ pm10: { values: [[18, "2026-09-15T09:00:00Z"], [12, "2026-09-15T08:00:00Z"]] } }, "pm10");
    expect(pairs.map((point) => point.value)).toEqual([12, 18]);
    const instances = pointsOf(
      { pm10: [{ value: 3, observedAt: "2026-09-15T08:00:00Z" }, { value: "x", observedAt: "2026-09-15T09:00:00Z" }] },
      "pm10",
    );
    expect(instances).toEqual([{ at: Date.parse("2026-09-15T08:00:00Z"), value: 3 }]);
    expect(pointsOf({}, "pm10")).toEqual([]);
    expect(polylineOf([])).toBe("");
    // One reading has no span; it draws at the top of the box rather than dividing by zero.
    expect(polylineOf([{ at: 1, value: 5 }])).toBe("0.00,40.00");
  });
});
