import { act, render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { MapLibreView } from "../src/components/dashboards/MapLibreView";
import type { MapLayer } from "../src/components/dashboards/MapLibreView";

interface Source {
  setData?: (data: unknown) => void;
  getClusterExpansionZoom?: (id: number) => Promise<number>;
}

// jsdom has no WebGL: the map is a recorder whose handlers the test can fire.
const rec = vi.hoisted(() => ({
  layers: [] as Record<string, unknown>[],
  removedLayers: [] as string[],
  removedSources: [] as string[],
  fitted: [] as unknown[],
  eased: [] as unknown[],
  plain: { styles: [] as unknown[] },
  handlers: new Map<string, () => void>(),
  clicks: new Map<string, (event: unknown) => void>(),
  onceHandlers: new Map<string, () => void>(),
  source: undefined as Source | undefined,
  throwOnConstruct: false,
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor() {
      if (rec.throwOnConstruct) throw new Error("WebGL is not supported");
    }
    addControl() {}
    addSource() {}
    addLayer(layer: Record<string, unknown>) {
      rec.layers.push(layer);
    }
    on(event: string, second: unknown, third?: unknown) {
      if (typeof second === "function") {
        if (event === "load") {
          (second as () => void)();
          return;
        }
        rec.handlers.set(event, second as () => void);
      }
      if (event === "click" && typeof second === "string" && typeof third === "function") {
        rec.clicks.set(second, third as (event: unknown) => void);
      }
    }
    once(event: string, handler: () => void) {
      rec.onceHandlers.set(event, handler);
    }
    remove() {}
    isStyleLoaded() {
      return false;
    }
    setStyle(style: unknown) {
      rec.plain.styles.push(style);
    }
    getSource() {
      return rec.source;
    }
    removeLayer(id: string) {
      rec.removedLayers.push(id);
    }
    removeSource(id: string) {
      rec.removedSources.push(id);
    }
    off() {}
    easeTo(options: unknown) {
      rec.eased.push(options);
    }
    fitBounds(...args: unknown[]) {
      rec.fitted.push(args);
    }
    getBounds() {
      return { getWest: () => 1, getSouth: () => 2, getEast: () => 3, getNorth: () => 4 };
    }
  }
  class FakePopup {
    setLngLat() {
      return this;
    }
    setDOMContent() {
      return this;
    }
    addTo() {
      return this;
    }
  }
  return {
    setWorkerUrl: () => {},
    Map: FakeMap,
    NavigationControl: class {},
    Popup: FakePopup,
    default: { Map: FakeMap, NavigationControl: class {}, Popup: FakePopup },
  };
});

const point = (x: number, y: number) => ({ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: [x, y] } });
const collection = (...features: unknown[]) => ({ type: "FeatureCollection", features });

const layer = (over: Partial<MapLayer> & { name: string }): MapLayer => ({ url: `/q/${over.name}`, style: "circle", ...over });

function view(layers: MapLayer[], props: { zoom?: number; onMoveEnd?: (box: number[]) => void } = {}) {
  return (
    <I18nextProvider i18n={i18n}>
      <MapLibreView layers={layers} label="Map" {...props} />
    </I18nextProvider>
  );
}

describe("MapLibreView", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    rec.layers.length = 0;
    rec.removedLayers.length = 0;
    rec.removedSources.length = 0;
    rec.fitted.length = 0;
    rec.eased.length = 0;
    rec.plain.styles.length = 0;
    rec.handlers.clear();
    rec.clicks.clear();
    rec.onceHandlers.clear();
    rec.source = undefined;
    rec.throwOnConstruct = false;
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("paints a line layer with a line colour and a fill layer with a fill colour", () => {
    render(view([layer({ name: "roads", style: "line" }), layer({ name: "zones", style: "fill" })]));
    const paint = (id: string) => rec.layers.find((l) => l.id === id)?.paint as Record<string, unknown>;
    expect(paint("roads")).toMatchObject({ "line-width": 2 });
    expect(paint("roads")).toHaveProperty("line-color");
    expect(paint("zones")).toMatchObject({ "fill-opacity": 0.5 });
    expect(paint("zones")).toHaveProperty("fill-color");
  });

  it("tells the caller the viewport after every move", () => {
    const moved = vi.fn();
    render(view([], { onMoveEnd: moved }));
    act(() => rec.handlers.get("moveend")?.());
    expect(moved).toHaveBeenCalledWith([1, 2, 3, 4]);
  });

  it("draws the layers once the plain ground's style loads after a failed basemap", () => {
    render(view([layer({ name: "stations" })]));
    act(() => rec.handlers.get("error")?.());
    expect(rec.plain.styles).toHaveLength(1);
    act(() => rec.onceHandlers.get("style.load")?.());
    expect(rec.layers.some((l) => l.id === "stations")).toBe(true);
  });

  it("shows the unavailable notice and keeps it when the props change afterwards", () => {
    rec.throwOnConstruct = true;
    const { rerender } = render(view([], { zoom: 5 }));
    expect(screen.getByRole("status")).toBeInTheDocument();
    rerender(view([], { zoom: 6 }));
    expect(screen.getByRole("status")).toBeInTheDocument();
  });

  it("replaces a source's data in place and fits the view to all layers together", () => {
    const setData = vi.fn();
    rec.source = { setData };
    const first = layer({ name: "a", data: collection(point(1, 2)) });
    const second = layer({ name: "b", data: collection(point(5, 9)) });
    const { rerender } = render(view([first, second]));
    expect(rec.fitted).toEqual([[[1, 2, 5, 9], { padding: 16, maxZoom: 14, duration: 0 }]]);
    const next = { ...first, data: collection(point(3, 3)) };
    rerender(view([next, second]));
    expect(setData).toHaveBeenCalledTimes(1);
    expect(setData).toHaveBeenCalledWith(next.data);
    rerender(view([next, second]));
    expect(setData).toHaveBeenCalledTimes(1);
  });

  it("zooms into a cluster that is clicked and ignores clicks it cannot place", async () => {
    const getClusterExpansionZoom = vi.fn(() => Promise.resolve(9));
    rec.source = { getClusterExpansionZoom };
    render(view([layer({ name: "buses", cluster: true })]));
    const click = rec.clicks.get("buses--clusters");
    expect(click).toBeDefined();
    const cluster = (id: unknown, geometry: unknown) => ({ features: [{ properties: { cluster_id: id }, geometry }] });
    const at = { type: "Point", coordinates: [17.1, 48.1] };
    click?.({});
    click?.(cluster("x", at));
    click?.(cluster(3, { type: "LineString", coordinates: [] }));
    rec.source = {};
    click?.(cluster(3, at));
    expect(getClusterExpansionZoom).not.toHaveBeenCalled();
    rec.source = { getClusterExpansionZoom };
    await act(async () => {
      click?.(cluster(3, at));
    });
    expect(getClusterExpansionZoom).toHaveBeenCalledWith(3);
    expect(rec.eased).toEqual([{ center: [17.1, 48.1], zoom: 9 }]);
  });

  it("removes a clustered layer together with its cluster circles", () => {
    const { rerender } = render(view([layer({ name: "buses", cluster: true })]));
    rerender(view([]));
    expect(rec.removedLayers).toEqual(["buses--clusters", "buses"]);
    expect(rec.removedSources).toEqual(["buses"]);
  });
});
