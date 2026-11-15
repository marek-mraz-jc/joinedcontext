import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { DistrictOutput } from "../districts";
import { DistrictMap } from "./DistrictMap";

type Handler = (event?: unknown) => void;

/** A MapLibre double that keeps every handler the map registers, and what it was given. */
const maps = vi.hoisted(() => ({
  made: [] as Array<{
    handlers: Record<string, Handler>;
    data: unknown[];
    fitted: unknown[];
    resized: number;
    canvas: { style: { cursor: string } };
    removed: boolean;
  }>,
}));

vi.mock("maplibre-gl", () => ({
  Map: class {
    state = { handlers: {} as Record<string, Handler>, data: [] as unknown[], fitted: [] as unknown[], resized: 0, canvas: { style: { cursor: "" } }, removed: false };
    constructor() {
      maps.made.push(this.state);
    }
    on(event: string, layerOrHandler: unknown, handler?: Handler) {
      this.state.handlers[typeof layerOrHandler === "string" ? `${event}:${layerOrHandler}` : event] = (handler ?? layerOrHandler) as Handler;
    }
    once(event: string, handler: Handler) {
      this.state.handlers[`once:${event}`] = handler;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    setPaintProperty = vi.fn();
    getSource = () => ({ setData: (data: unknown) => this.state.data.push(data) });
    fitBounds = (bounds: unknown) => this.state.fitted.push(bounds);
    resize = () => {
      this.state.resized += 1;
    };
    getCanvas = () => this.state.canvas;
    remove = () => {
      this.state.removed = true;
      this.state.handlers["once:remove"]?.();
    };
  },
  setWorkerUrl: vi.fn(),
}));

/** The ResizeObserver the map watches its box with, kept so a test can say the box changed. */
let resized: (() => void) | null = null;
let disconnected = 0;

beforeEach(() => {
  maps.made.length = 0;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resized = callback;
      }
      observe() {}
      disconnect() {
        disconnected += 1;
      }
    },
  );
});
afterEach(() => vi.unstubAllGlobals());

const district = (code: string, name: string, events: number, pm25: number | null): DistrictOutput => ({
  code,
  name,
  areaKm2: 1,
  events,
  bikes: 0,
  bikeSlots: 0,
  alerts: 0,
  pm25,
  aqi: null,
  perKm2: { events, bikes: 0, bikeSlots: 0, alerts: 0 },
  rank: { events: 1, bikes: null, bikeSlots: null, alerts: null, pm25: null, aqi: null },
});
const SQUARE = (x: number) => ({ type: "Polygon", coordinates: [[[x, 60.1], [x + 0.01, 60.1], [x + 0.01, 60.11], [x, 60.1]]] });
const DISTRICTS = [district("001", "Kamppi", 4, 7), district("002", "Kallio", 2, null), district("003", "Nowhere", 1, null)];
// The third district has no outline: it is compared, never drawn.
const GEOMETRIES = new Map<string, unknown>([["001", SQUARE(24.93)], ["002", { type: "MultiPolygon", coordinates: [[[[24.95, 60.18], [24.96, 60.18], [24.96, 60.19], [24.95, 60.18]]]] }]]);

function draw(onSelect = vi.fn(), basemap?: string, measure: "events" | "pm25" = "events") {
  const view = render(
    <JcProvider client={stubClient({}, { appName: "district-compare", ...(basemap ? { basemap } : {}) })}>
      <DistrictMap districts={DISTRICTS} geometries={GEOMETRIES} selectedCodes={["001"]} measure={measure} onToggle={onSelect} label="Map" />
    </JcProvider>,
  );
  return { view, onSelect };
}

describe("the district map", () => {
  it("draws the districts that have an outline, the chosen one marked, and fits them once", async () => {
    draw();
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    map.handlers.load();
    await waitFor(() => expect(map.data).toHaveLength(1));
    const drawn = map.data[0] as { features: Array<{ id: string; properties: { selected: number; hasValue: boolean } }> };
    expect(drawn.features.map((feature) => feature.id)).toEqual(["001", "002"]);
    expect(drawn.features[0].properties.selected).toBe(1);
    expect(map.fitted).toHaveLength(1);
  });

  it("marks a district with no value for the measure as having none", async () => {
    draw(vi.fn(), undefined, "pm25");
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    map.handlers.load();
    await waitFor(() => expect(map.data).toHaveLength(1));
    const drawn = map.data[0] as { features: Array<{ properties: { hasValue: boolean } }> };
    expect(drawn.features.map((feature) => feature.properties.hasValue)).toEqual([true, false]);
  });

  it("picks a district by its area, ignores a click on nothing, and shows a pointer over an area", async () => {
    const { onSelect } = draw();
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    map.handlers.load();
    map.handlers["click:districts-fill"]({ features: [] });
    expect(onSelect).not.toHaveBeenCalled();
    map.handlers["click:districts-fill"]({ features: [{ properties: { code: "002" } }] });
    expect(onSelect).toHaveBeenCalledWith("002");
    map.handlers["mouseenter:districts-fill"]();
    expect(map.canvas.style.cursor).toBe("pointer");
    map.handlers["mouseleave:districts-fill"]();
    expect(map.canvas.style.cursor).toBe("");
  });

  it("keeps its fit when the box changes, says a map error, and lets go of the box when removed", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { view } = draw(vi.fn(), "https://tiles.example.org/style.json");
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    resized?.();
    expect(map.resized).toBe(1);
    map.handlers.load();
    await waitFor(() => expect(map.fitted).toHaveLength(1));
    resized?.();
    expect(map.resized).toBe(2);
    expect(map.fitted).toHaveLength(2);
    map.handlers.error({ error: { message: "tile refused" } });
    map.handlers.error({});
    expect(errors).toHaveBeenCalledWith("district-compare: map error", "tile refused");
    const before = disconnected;
    view.unmount();
    expect(map.removed).toBe(true);
    expect(disconnected).toBe(before + 1);
    // Gone: a late resize or load does nothing.
    resized?.();
    map.handlers.load();
    expect(map.resized).toBe(2);
  });

  it("makes no map when it is gone before the map's worker is ready", async () => {
    const { view } = draw();
    view.unmount();
    await new Promise((settle) => setTimeout(settle, 0));
    expect(maps.made).toHaveLength(0);
  });
});
