import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { STATION_1, STATION_2, STATION_3 } from "../fixtures/bikes";
import { StationMap } from "./StationMap";

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

const NOWHERE = { ...STATION_2, id: "urn:ngsi-ld:BikeHireDockingStation:nowhere", location: null };

function draw(onSelect = vi.fn(), basemap?: string) {
  const view = render(
    <JcProvider client={stubClient({}, { appName: "bike-weather-demand", ...(basemap ? { basemap } : {}) })}>
      <StationMap stations={[STATION_1, STATION_3, NOWHERE]} selectedId={STATION_1.id} onSelect={onSelect} label="Stations on map" />
    </JcProvider>,
  );
  return { view, onSelect };
}

describe("the station map", () => {
  it("draws the stations that have a position, the chosen one marked, and fits them once", async () => {
    draw();
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    map.handlers.load();
    await waitFor(() => expect(map.data).toHaveLength(1));
    const drawn = map.data[0] as { features: Array<{ id: string; properties: { selected: number } }> };
    expect(drawn.features.map((feature) => feature.id)).toEqual([STATION_1.id, STATION_3.id]);
    expect(drawn.features[0].properties.selected).toBe(1);
    expect(map.fitted).toHaveLength(1);
  });

  it("picks a station by its circle, ignores a click on nothing, and shows a pointer over a circle", async () => {
    const { onSelect } = draw();
    await waitFor(() => expect(maps.made).toHaveLength(1));
    const [map] = maps.made;
    map.handlers.load();
    map.handlers["click:stations-circle"]({ features: [] });
    expect(onSelect).not.toHaveBeenCalled();
    map.handlers["click:stations-circle"]({ features: [{ properties: { id: STATION_3.id } }] });
    expect(onSelect).toHaveBeenCalledWith(STATION_3.id);
    map.handlers["mouseenter:stations-circle"]();
    expect(map.canvas.style.cursor).toBe("pointer");
    map.handlers["mouseleave:stations-circle"]();
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
    expect(errors).toHaveBeenCalledWith("bike-weather-demand: map error", "tile refused");
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
