// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/dashboards/DeckGlOverlay.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import type { Feature } from "geojson";
import { HeatmapLayer, HexagonLayer } from "@deck.gl/aggregation-layers";
import { ScatterplotLayer } from "@deck.gl/layers";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { DECK_GL_THRESHOLD, rendersWithDeckGl } from "../src/components/dashboards/rendering";

// jsdom has no WebGL, so the map and the overlay are recorders: what is asserted is which
// renderer each layer was handed to, which is the whole of UI-21.
const calls = vi.hoisted(() => ({
  constructed: [] as Record<string, unknown>[],
  sources: [] as { id: string; source: Record<string, unknown> }[],
  controls: [] as unknown[],
  removed: [] as unknown[],
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(options: Record<string, unknown>) {
      calls.constructed.push(options);
    }
    addControl(control: unknown) {
      calls.controls.push(control);
    }
    removeControl(control: unknown) {
      calls.removed.push(control);
    }
    addSource(id: string, source: Record<string, unknown>) {
      calls.sources.push({ id, source });
    }
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
      return { getWest: () => 0, getSouth: () => 0, getEast: () => 1, getNorth: () => 1 };
    }
  }
  const NavigationControl = class {};
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
  return {
    setWorkerUrl: () => undefined,
    Map: FakeMap,
    NavigationControl,
    Popup,
    default: { Map: FakeMap, NavigationControl, Popup },
  };
});

// The overlay itself is deck.gl's `Deck` behind a control interface, which needs a canvas.
// The layers it is given are real, so the assertions below are about real deck.gl classes.
vi.mock("@deck.gl/mapbox", () => ({
  MapboxOverlay: class {
    layers: unknown[];
    constructor(props: { layers: unknown[] }) {
      this.layers = props.layers;
    }
    setProps(props: { layers: unknown[] }) {
      this.layers = props.layers;
    }
  },
}));

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "doprava", namespace: "banskabystrica" },
  spec: {
    title: { sk: "Doprava", en: "Traffic" },
    visibility: "project",
    pages: [{ title: "Mapa", layout: "full-map", layers: ["parking-telemetry"] }],
  },
  status: { phase: "Live" },
};

function layerManifest(style: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Layer",
    metadata: { name: "parking-telemetry", namespace: "banskabystrica" },
    spec: {
      sourceEndpointRef: "public-parking",
      entityType: "ParkingSpot",
      style,
      colorBy: { property: "occupancy", palette: "YlOrRd", domain: [0, 100] },
      sizeBy: { property: "occupancy", range: [4, 18] },
    },
    status: { phase: "Live" },
  };
}

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "public-parking", namespace: "banskabystrica" },
  spec: {
    contextSpaceRef: "doprava",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "csv"],
  },
  status: { phase: "Live" },
};

/** A telemetry point, cheap enough that fifty thousand of them cost a test nothing. */
function spot(index: number): Feature {
  return {
    type: "Feature",
    geometry: { type: "Point", coordinates: [19.14 + index * 1e-6, 48.73] },
    properties: { occupancy: index % 100 },
  };
}

function renderDashboard(options: { style?: string; features?: number } = {}) {
  const list = (items: unknown[]) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items,
  });
  const features = Array.from({ length: options.features ?? 3 }, (_, index) => spot(index));

  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const path = new URL(url, window.location.origin).pathname;
      if (path.endsWith("/ngsi-ld/v1/entities")) {
        // Not a real `Response`: serialising fifty thousand features would cost more than
        // the assertion is worth, and every consumer here only calls `.json()`.
        return Promise.resolve({
          ok: true,
          status: 200,
          json: () => Promise.resolve({ type: "FeatureCollection", features }),
        } as Response);
      }
      let body: unknown = list([]);
      if (path.endsWith("/auth/me")) {
        body = IDENTITY;
      } else if (path.endsWith("/dashboards")) {
        body = list([DASHBOARD]);
      } else if (path.endsWith("/layers")) {
        body = list([layerManifest(options.style ?? "circle")]);
      } else if (path.endsWith("/endpoints")) {
        body = list([ENDPOINT]);
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

/** The layers of the overlay the map was given, or nothing when it was given none. */
function overlaid(): unknown[] {
  const overlay = calls.controls.find((control) => "layers" in (control as object));
  return (overlay as { layers: unknown[] } | undefined)?.layers ?? [];
}

describe("the 50k feature threshold (UI-21)", () => {
  it("keeps a small point layer on MapLibre and hands a large one to deck.gl", () => {
    expect(rendersWithDeckGl("circle", DECK_GL_THRESHOLD - 1)).toBe(false);
    expect(rendersWithDeckGl("circle", DECK_GL_THRESHOLD)).toBe(true);
  });

  it("hands an aggregation to deck.gl however few features it has", () => {
    expect(rendersWithDeckGl("hexagon", 3)).toBe(true);
    expect(rendersWithDeckGl("heatmap", 0)).toBe(true);
    expect(rendersWithDeckGl("fill", 12)).toBe(false);
  });
});

describe("deck.gl overlay", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/dashboards");
    calls.constructed.length = 0;
    calls.sources.length = 0;
    calls.controls.length = 0;
    calls.removed.length = 0;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("mounts the map and draws a dense layer with a deck.gl overlay above it", async () => {
    renderDashboard({ features: DECK_GL_THRESHOLD });

    expect(await screen.findByRole("application", { name: "Traffic" })).toBeInTheDocument();
    await waitFor(() => expect(overlaid()).toHaveLength(1));
    const [layer] = overlaid();
    expect(layer).toBeInstanceOf(ScatterplotLayer);
    expect((layer as ScatterplotLayer).id).toBe("parking-telemetry");
    expect(
      calls.sources.map((entry) => entry.id),
      "a layer the overlay draws is not added to MapLibre a second time",
    ).toEqual([]);
  });

  it("leaves a layer below the threshold to MapLibre", async () => {
    renderDashboard({ features: 3 });

    await waitFor(() => expect(calls.sources).toHaveLength(1));
    expect(calls.sources[0].id).toBe("parking-telemetry");
    expect(overlaid()).toEqual([]);
  });

  it("hands the features it already fetched to MapLibre instead of the URL", async () => {
    renderDashboard({ features: 3 });

    await waitFor(() => expect(calls.sources).toHaveLength(1));
    const data = calls.sources[0].source.data as { type: string; features: unknown[] };
    expect(data).toMatchObject({ type: "FeatureCollection" });
    expect(data.features).toHaveLength(3);
  });

  it("aggregates a hexagon layer whatever its size", async () => {
    renderDashboard({ style: "hexagon", features: 3 });

    await waitFor(() => expect(overlaid()).toHaveLength(1));
    expect(overlaid()[0]).toBeInstanceOf(HexagonLayer);
    expect(calls.sources).toEqual([]);
  });

  it("aggregates a heatmap layer the same way", async () => {
    renderDashboard({ style: "heatmap", features: 3 });

    await waitFor(() => expect(overlaid()).toHaveLength(1));
    expect(overlaid()[0]).toBeInstanceOf(HeatmapLayer);
  });
});
