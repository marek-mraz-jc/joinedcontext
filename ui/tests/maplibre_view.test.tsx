import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { MapLibreView } from "../src/components/dashboards/MapLibreView";
import { ONE_VALUE_COLOUR } from "../src/components/dashboards/mapColours";

// jsdom has no WebGL, so the map itself is a recorder: the assertions are about what the
// view asks MapLibre for — the sources, the layers, the click handlers.
const calls = vi.hoisted(() => ({
  constructed: [] as Record<string, unknown>[],
  sources: [] as { id: string; source: Record<string, unknown> }[],
  layers: [] as Record<string, unknown>[],
  clickHandlers: new Map<string, (event: unknown) => void>(),
  popups: [] as { html: string; node?: HTMLElement }[],
  errorHandlers: [] as (() => void)[],
  styles: [] as unknown[],
  once: [] as string[],
  throwOnConstruct: false,
  workerUrls: [] as string[],
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(options: Record<string, unknown>) {
      if (calls.throwOnConstruct) {
        throw new Error("WebGL is not supported");
      }
      calls.constructed.push(options);
    }
    addControl() {}
    addSource(id: string, source: Record<string, unknown>) {
      calls.sources.push({ id, source });
    }
    addLayer(layer: Record<string, unknown>) {
      calls.layers.push(layer);
    }
    on(event: string, second: unknown, third?: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
        return;
      }
      if (event === "error" && typeof second === "function") {
        calls.errorHandlers.push(second as () => void);
        return;
      }
      if (event === "click" && typeof second === "string" && typeof third === "function") {
        calls.clickHandlers.set(second, third as (event: unknown) => void);
      }
    }
    once(event: string) {
      calls.once.push(event);
    }
    remove() {}
    isStyleLoaded() {
      return false;
    }
    setStyle(style: unknown) {
      calls.styles.push(style);
    }
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    off() {}
    easeTo() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 0, getSouth: () => 0, getEast: () => 1, getNorth: () => 1 };
    }
  }
  class FakePopup {
    setLngLat() {
      return this;
    }
    setHTML(html: string) {
      calls.popups.push({ html });
      return this;
    }
    setDOMContent(node: HTMLElement) {
      calls.popups.push({ html: node.innerHTML, node });
      return this;
    }
    addTo() {
      return this;
    }
  }
  return {
    setWorkerUrl: (url: string) => {
      calls.workerUrls.push(url);
    },
    Map: FakeMap,
    NavigationControl: class {},
    Popup: FakePopup,
    default: { Map: FakeMap, NavigationControl: class {}, Popup: FakePopup },
  };
});

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

function dashboard(visibility: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Dashboard",
    metadata: { name: "ovzdusie-prehlad", namespace: "banskabystrica" },
    spec: {
      title: { sk: "Kvalita ovzdušia", en: "Air quality" },
      visibility,
      pages: [{ title: "Mapa staníc", layout: "full-map", layers: ["air-quality-stations"] }],
    },
    status: { phase: "Live" },
  };
}

const LAYER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Layer",
  metadata: { name: "air-quality-stations", namespace: "banskabystrica" },
  spec: {
    sourceEndpointRef: "public-air",
    entityType: "AirQualityObserved",
    style: "circle",
    filter: { q: "pm10>0", scopeQ: "/geo/SK/BB/#" },
    colorBy: { property: "pm10", palette: "YlOrRd", domain: [0, 100] },
    sizeBy: { property: "pm10", range: [4, 18] },
    popupProperties: ["stationName", "pm10"],
  },
  status: { phase: "Live" },
};

function endpoint(audience: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience,
      enabledRepresentations: ["ngsi-ld", "csv"],
    },
    status: { phase: "Live" },
  };
}

function renderDashboards(options: { visibility?: string; audience?: string } = {}) {
  const list = (items: unknown[]) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items,
  });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const path = new URL(url, window.location.origin).pathname;
      let body: unknown = list([]);
      if (path.endsWith("/auth/me")) {
        body = IDENTITY;
      } else if (path.endsWith("/dashboards")) {
        body = list([dashboard(options.visibility ?? "project")]);
      } else if (path.endsWith("/layers")) {
        body = list([LAYER]);
      } else if (path.endsWith("/endpoints")) {
        body = list([endpoint(options.audience ?? "public")]);
      }
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }),
  );

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("map dashboard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/dashboards");
    calls.constructed.length = 0;
    calls.errorHandlers.length = 0;
    calls.styles.length = 0;
    calls.once.length = 0;
    calls.sources.length = 0;
    calls.layers.length = 0;
    calls.popups.length = 0;
    calls.clickHandlers.clear();
    calls.throwOnConstruct = false;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // T-2760: MapLibre looked for its worker beside its own module, /assets/maplibre-gl-worker.mjs,
  // which the build never emitted; the view hands it the worker Vite bundled instead.
  it("hands MapLibre the address of the bundled worker before any map is made", async () => {
    await import("../src/components/dashboards/MapLibreView");
    expect(calls.workerUrls).toHaveLength(1);
    expect(calls.workerUrls[0]).not.toMatch(/maplibre-gl-worker\.mjs$/);
    expect(calls.workerUrls[0]).toMatch(/\.(js|mjs|ts)(\?.*)?$/);
  });

  it("mounts a map container labelled with the dashboard title", async () => {
    renderDashboards();
    expect(
      await screen.findByRole("application", { name: "Air quality" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(calls.constructed).toHaveLength(1));
  });

  it("draws the ground from the project's basemap route on the Portal's origin, never a third-party style (AP-67)", async () => {
    renderDashboards();
    await waitFor(() => expect(calls.constructed).toHaveLength(1));
    const style = calls.constructed[0].style as string;
    expect(style).toBe(
      `${window.location.origin}/api/v1/projects/banskabystrica/basemap/default/style.json`,
    );
    expect(style).not.toContain("demotiles");

    // A platform without a basemap answers the style with an error: the map falls back to the
    // plain ground once instead of waiting for a style that never loads.
    calls.errorHandlers.forEach((handler) => handler());
    calls.errorHandlers.forEach((handler) => handler());
    expect(calls.styles).toHaveLength(1);
    expect(calls.styles[0]).toMatchObject({ version: 8, sources: {} });
    // The map's own `load` never comes after its first style failed: the plain ground's
    // `style.load` is what lets the layers be added (T-3256).
    expect(calls.once).toEqual(["style.load"]);
  });

  it("loads the layer as GeoJSON from the endpoint's NGSI-LD API, filters included (UI-22, T-4600)", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(calls.sources[0].id).toBe("air-quality-stations");
    expect(calls.sources[0].source).toMatchObject({ type: "geojson" });
    const url = new URL(String(calls.sources[0].source.data), window.location.origin);
    expect(url.pathname).toBe("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/entities");
    expect(url.searchParams.get("options")).toBe("keyValues");
    expect(url.searchParams.get("type")).toBe("AirQualityObserved");
    expect(url.searchParams.get("q")).toBe("pm10>0");
    expect(url.searchParams.get("scopeQ")).toBe("/geo/SK/BB/#");

    // The page's own read asks for GeoJSON; no request goes to a file the Endpoint no longer has.
    const requests = vi.mocked(fetch).mock.calls;
    const layerRead = requests.find(([input]) => String(input).includes("/ngsi-ld/v1/entities"));
    expect(new Headers(layerRead?.[1]?.headers).get("Accept")).toBe("application/geo+json");
    expect(requests.some(([input]) => String(input).includes("/file."))).toBe(false);

    // And so does MapLibre, when it fetches a layer by URL itself; its other requests are untouched.
    const transform = calls.constructed[0].transformRequest as (url: string, kind: string) => { headers?: Record<string, string> };
    expect(transform(url.href, "Source").headers).toEqual({ Accept: "application/geo+json" });
    expect(transform("https://tiles.example/style.json", "Style").headers).toBeUndefined();
  });

  it("styles the points from colorBy and sizeBy", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.layers).toHaveLength(1));
    const paint = calls.layers[0].paint as Record<string, unknown>;
    expect(calls.layers[0]).toMatchObject({ id: "air-quality-stations", type: "circle" });
    expect(JSON.stringify(paint["circle-color"])).toContain("pm10");
    // T-6442: a feature without the number is unknown, not the ramp's low end.
    expect(JSON.stringify(paint["circle-color"])).toContain('["has","pm10"]');
    expect(JSON.stringify(paint["circle-radius"])).toContain("18");
  });

  it("opens a popup with the layer's properties on a feature click", async () => {
    renderDashboards();

    await waitFor(() => expect(calls.clickHandlers.size).toBe(1));
    calls.clickHandlers.get("air-quality-stations")?.({
      lngLat: { lng: 19.1, lat: 48.7 },
      features: [{ properties: { stationName: "Štiavničky", pm10: 42, secret: "hidden" } }],
    });

    expect(calls.popups).toHaveLength(1);
    expect(calls.popups[0].html).toContain("Štiavničky");
    expect(calls.popups[0].html).toContain("42");
    // Only the properties the Layer manifest names are shown.
    expect(calls.popups[0].html).not.toContain("hidden");
  });

  it("refuses a private endpoint behind a public dashboard (UI-19)", async () => {
    renderDashboards({ visibility: "public", audience: "organization" });

    expect(await screen.findByText(/only read public endpoints/)).toBeInTheDocument();
    await waitFor(() => expect(calls.constructed).toHaveLength(1));
    expect(calls.sources).toHaveLength(0);
  });

  it("says so instead of blanking the page when WebGL is missing", async () => {
    calls.throwOnConstruct = true;
    renderDashboards();

    expect(await screen.findByText(en.dashboards.mapUnavailable)).toBeInTheDocument();
  });

  it("lists the layers with what colours them", async () => {
    renderDashboards();

    const legend = await screen.findByText("air-quality-stations");
    expect(within(legend.closest("li") as HTMLElement).getByText(/pm10/)).toBeInTheDocument();
  });
});

describe("a clustered point layer (T-3256)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    calls.sources.length = 0;
    calls.layers.length = 0;
    calls.popups.length = 0;
    calls.clickHandlers.clear();
    calls.throwOnConstruct = false;
  });

  it("paints a layer of one value one colour, with no interpolate at all", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MapLibreView
          label="Map of BikeStation"
          layers={[
            {
              name: "one-value",
              url: "",
              data: { type: "FeatureCollection", features: [] },
              style: "circle",
              colorBy: { property: "bikes", domain: [0, 0] },
              sizeBy: { property: "bikes", range: [4, 18] },
            },
          ]}
        />
      </I18nextProvider>,
    );

    await waitFor(() => expect(calls.layers.length).toBeGreaterThan(0));
    const paint = calls.layers[0].paint as Record<string, unknown>;
    expect(JSON.stringify(paint)).not.toContain("interpolate");
    expect(paint["circle-color"]).toEqual(["case", ["has", "bikes"], ONE_VALUE_COLOUR, expect.any(String)]);
    expect(paint["circle-radius"]).toBe(4);
  });

  it("clusters the points, draws the clusters apart, and opens a popup with the entity's button", async () => {
    const opened: string[] = [];
    render(
      <I18nextProvider i18n={i18n}>
        <MapLibreView
          label="Map of BikeStation"
          layers={[
            {
              name: "explore-entities",
              url: "",
              data: { type: "FeatureCollection", features: [] },
              style: "circle",
              cluster: true,
              popupProperties: ["name"],
              open: { label: "Open the entity", onOpen: (id) => opened.push(id) },
            },
          ]}
        />
      </I18nextProvider>,
    );
    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(calls.sources[0].source).toMatchObject({ cluster: true, clusterRadius: 48, clusterMaxZoom: 14 });
    expect(calls.layers.map((layer) => [layer.id, layer.filter])).toEqual([
      ["explore-entities", ["!", ["has", "point_count"]]],
      ["explore-entities--clusters", ["has", "point_count"]],
    ]);
    calls.clickHandlers.get("explore-entities")?.({
      lngLat: { lng: 24.9, lat: 60.2 },
      features: [{ properties: { id: "urn:ngsi-ld:BikeStation:1", name: "Kamppi" } }],
    });
    const node = calls.popups[0].node as HTMLElement;
    expect(node.textContent).toContain("Kamppi");
    within(node).getByRole("button", { name: "Open the entity" }).click();
    expect(opened).toEqual(["urn:ngsi-ld:BikeStation:1"]);
  });
});
