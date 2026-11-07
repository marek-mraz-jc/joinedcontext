// covers: src/components/dashboards/MapLibreView.tsx src/components/dashboards/DeckGlOverlay.tsx
/**
 * UI-22: a refetch after a pan keeps the map the reader moved (F-UI1-015), and a layer that leaves
 * and comes back opens one popup per click (F-UI1-016).
 */
import { render } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Feature } from "geojson";
import i18n from "../src/i18n";

const calls = vi.hoisted(() => ({
  maps: 0,
  popups: 0,
  overlays: [] as { layers: unknown[] }[],
  handlers: new Map<string, Set<(event: unknown) => void>>(),
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor() {
      calls.maps += 1;
    }
    addControl() {}
    removeControl() {}
    addSource() {}
    addLayer() {}
    on(event: string, second: unknown, third?: unknown) {
      if (event === "load" && typeof second === "function") (second as () => void)();
      if (event === "click" && typeof second === "string") {
        const set = calls.handlers.get(second) ?? new Set();
        set.add(third as (event: unknown) => void);
        calls.handlers.set(second, set);
      }
    }
    off(event: string, layer: string, handler: (event: unknown) => void) {
      if (event === "click") calls.handlers.get(layer)?.delete(handler);
    }
    once() {}
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    fitBounds() {}
    isStyleLoaded() {
      return true;
    }
    getBounds() {
      return { getWest: () => 0, getSouth: () => 0, getEast: () => 1, getNorth: () => 1 };
    }
  }
  class Popup {
    constructor() {
      calls.popups += 1;
    }
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
  return { setWorkerUrl: () => undefined, Map: FakeMap, NavigationControl: class {}, Popup };
});

vi.mock("@deck.gl/mapbox", () => ({
  MapboxOverlay: class {
    layers: unknown[];
    constructor(props: { layers: unknown[] }) {
      this.layers = props.layers;
      calls.overlays.push(this);
    }
    setProps(props: { layers: unknown[] }) {
      this.layers = props.layers;
    }
  },
}));

const { MapLibreView } = await import("../src/components/dashboards/MapLibreView");
const { DeckGlOverlay } = await import("../src/components/dashboards/DeckGlOverlay");

const point = (n: number): Feature => ({ type: "Feature", geometry: { type: "Point", coordinates: [17 + n / 1000, 48] }, properties: { n } });
const dense = (count: number) => [{ name: "telemetry", url: "/x", style: "circle" as const, features: Array.from({ length: count }, (_, n) => point(n)) }];
const layer = { name: "stops", url: "/stops", style: "circle" as const, data: { type: "FeatureCollection", features: [point(1)] } };

const wrap = (node: React.ReactNode) => <I18nextProvider i18n={i18n}>{node}</I18nextProvider>;

describe("the dashboard map", () => {
  beforeEach(() => {
    calls.maps = 0;
    calls.popups = 0;
    calls.overlays = [];
    calls.handlers.clear();
  });

  it("hands new dense features to the overlay instead of building a new map", () => {
    const view = render(wrap(<DeckGlOverlay layers={[]} dense={dense(3)} label="Traffic" />));
    view.rerender(wrap(<DeckGlOverlay layers={[]} dense={dense(5)} label="Traffic" />));
    expect(calls.maps).toBe(1);
    expect(calls.overlays).toHaveLength(1);
    expect(calls.overlays[0]?.layers).toHaveLength(1);
  });

  it("opens one popup per click on a layer that left and came back", () => {
    const view = render(wrap(<MapLibreView layers={[layer]} label="Stops" />));
    view.rerender(wrap(<MapLibreView layers={[]} label="Stops" />));
    view.rerender(wrap(<MapLibreView layers={[layer]} label="Stops" />));
    for (const handler of calls.handlers.get("stops") ?? []) {
      handler({ features: [{ properties: { n: 1 }, geometry: point(1).geometry }], lngLat: { lng: 17, lat: 48 } });
    }
    expect(calls.popups).toBe(1);
  });
});
