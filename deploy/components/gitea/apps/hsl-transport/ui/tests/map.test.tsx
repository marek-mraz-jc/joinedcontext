/**
 * The map is the app (T-0309, AP-38, AP-41). What matters is that a bus moving on the
 * Endpoint reaches the one GeoJSON source the map draws from, without the layer being rebuilt
 * and without the browser ever learning the Endpoint URL.
 *
 * MapLibre needs WebGL, which jsdom does not have, so the library is replaced by a double
 * that records what the app asked it to do. That is the whole contract worth asserting here:
 * one source, one `setData` per fleet.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setData = vi.fn();
const addSource = vi.fn();
const addLayer = vi.fn();
const remove = vi.fn();
const addImage = vi.fn();
let load: (() => void) | undefined;
let constructed: { style: unknown } | undefined;
let moveend: (() => void) | undefined;
/** Where the double says the map looks: all of Helsinki unless a test pans it. */
let view: [[number, number], [number, number]] = [[24.5, 60.0], [25.5, 60.4]];

vi.mock("maplibre-gl", () => {
  class Map {
    constructor(public options: unknown) {
      constructed = options as { style: unknown };
    }
    on(event: string, handler: () => void) {
      if (event === "load") {
        load = handler;
      }
      if (event === "moveend") {
        moveend = handler;
      }
    }
    addSource = addSource;
    addLayer = addLayer;
    addImage = addImage;
    hasImage = () => false;
    resize = () => undefined;
    getBounds = () => ({ toArray: () => view });
    getSource = () => ({ setData });
    remove = remove;
  }
  // Named, not default: maplibre-gl 6 has no default export.
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App, arrowImage, basemapOf, VehicleMap } from "../src/App";
import { featureCollection, inView, lineColor, NO_LINE, perLine, speedBands, type Vehicle } from "../src/api";
import { DEFAULT_TOKENS } from "@joinedcontext/sdk";

const BUS = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-01";
const OTHER = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-02";

function bus(id: string, longitude: number, refLine = "550"): Vehicle {
  return { id, coordinates: [longitude, 60.17], bearing: 143, speed: 8.5, refLine };
}

/** The stream, as the app's own backend serves it. */
class StubEventSource {
  static last: StubEventSource | undefined;
  closed = false;
  private listeners: Array<[string, (event: MessageEvent<string>) => void]> = [];
  constructor(readonly url: string) {
    StubEventSource.last = this;
  }
  addEventListener(type: string, handler: (event: MessageEvent<string>) => void) {
    this.listeners.push([type, handler]);
  }
  close() {
    this.closed = true;
  }
  private emit(type: string, data: unknown) {
    const event = new MessageEvent(type, { data: JSON.stringify(data) });
    for (const [listening, handler] of this.listeners) {
      if (listening === type) handler(event);
    }
  }
  /** What the server pushes when a bus moves. */
  push(vehicles: Vehicle[]) {
    this.emit("vehicles", vehicles);
  }
  /** What the server pushes when its live feed is lost, or live again (T-3581). */
  feed(lostSince: string | null) {
    this.emit("feed", { lostSince });
  }
}

beforeEach(() => {
  setData.mockClear();
  addSource.mockClear();
  addLayer.mockClear();
  remove.mockClear();
  addImage.mockClear();
  load = undefined;
  moveend = undefined;
  constructed = undefined;
  document.getElementById("jc-config")?.remove();
  view = [[24.5, 60.0], [25.5, 60.4]];
  StubEventSource.last = undefined;
  vi.stubGlobal("EventSource", StubEventSource);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify([bus(BUS, 24.94)]), { status: 200 })),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the map source", () => {
  it("is created once, with the fleet the app already had", () => {
    render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    load?.();

    // The alerts' stops first, under the vehicles (T-3895).
    expect(addSource.mock.calls.map(([name]) => name)).toEqual(["alerts", "vehicles"]);
    const [, source] = addSource.mock.calls[1] as [string, { type: string; data: unknown }];
    expect(source.type).toBe("geojson");
    // The line's colour, and the heading on top of it (T-2926).
    expect(addLayer).toHaveBeenCalledTimes(3);
    const [, dots, heading] = addLayer.mock.calls.map(([layer]) => layer as { id: string; paint?: Record<string, unknown>; layout?: Record<string, unknown> });
    expect(dots.paint?.["circle-color"]).toEqual(["get", "color"]);
    expect(heading.layout?.["icon-rotate"]).toEqual(["get", "bearing"]);
    expect(heading.layout?.["icon-rotation-alignment"]).toBe("map");
    expect(addImage).toHaveBeenCalledTimes(1);
  });

  it("takes a new fleet through setData rather than a second source", () => {
    const { rerender } = render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    load?.();
    setData.mockClear();

    rerender(<VehicleMap collection={featureCollection([bus(BUS, 24.99)])} />);

    // The fleet's source, once; the alert layer's (empty here) is written on its own.
    const fleets = setData.mock.calls
      .map(([data]) => data as ReturnType<typeof featureCollection>)
      .filter((data) => data.features.length > 0);
    expect(fleets).toHaveLength(1);
    const collection = fleets[0];
    expect(collection.features[0].geometry.coordinates).toEqual([24.99, 60.17]);
    expect(addSource).toHaveBeenCalledTimes(2);
    expect(addLayer).toHaveBeenCalledTimes(3);
  });

  it("is not written to before the map has loaded", () => {
    const { rerender } = render(<VehicleMap collection={featureCollection([bus(BUS, 24.94)])} />);
    rerender(<VehicleMap collection={featureCollection([bus(BUS, 24.99)])} />);

    expect(setData).not.toHaveBeenCalled();
  });
});

describe("the app", () => {
  it("moves a bus on the map when the stream says it moved", async () => {
    render(<App />);
    load?.();
    await waitFor(() => expect(StubEventSource.last).toBeDefined());
    await waitFor(() => expect(setData).toHaveBeenCalled());
    setData.mockClear();

    StubEventSource.last?.push([bus(BUS, 25.01)]);

    await waitFor(() => expect(setData).toHaveBeenCalled());
    const collection = setData.mock.lastCall?.[0] as ReturnType<typeof featureCollection>;
    expect(collection.features).toHaveLength(1);
    expect(collection.features[0].geometry.coordinates).toEqual([25.01, 60.17]);
  });

  it("merges a partial update instead of dropping the buses it does not mention", async () => {
    render(<App />);
    load?.();
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    StubEventSource.last?.push([bus(BUS, 24.94), bus(OTHER, 24.8, "551")]);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("2 vehicles"));

    StubEventSource.last?.push([bus(OTHER, 24.85, "551")]);

    await waitFor(() => {
      const collection = setData.mock.lastCall?.[0] as ReturnType<typeof featureCollection>;
      expect(collection.features).toHaveLength(2);
    });
    expect(screen.getByRole("status")).toHaveTextContent("2 vehicles, updating live");
  });

  it("says since when the live feed is lost, and updating live once it is back (T-3581)", async () => {
    render(<App />);
    load?.();
    await waitFor(() => expect(StubEventSource.last).toBeDefined());
    StubEventSource.last?.push([bus(BUS, 24.94)]);
    StubEventSource.last?.feed("2026-10-12T08:05:00Z");
    // 08:05 UTC is 11:05 in Helsinki in October.
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 vehicles, live feed lost since 11:05"));
    StubEventSource.last?.feed(null);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 vehicles, updating live"));
  });

  it("subscribes to its own backend and never to the endpoint", async () => {
    render(<App />);
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    expect(StubEventSource.last?.url).toBe("/api/stream");
    const calls = (globalThis.fetch as unknown as { mock: { calls: string[][] } }).mock.calls;
    for (const [url] of calls) {
      expect(url).toMatch(/^\/api\//);
    }
  });

  it("closes the stream when the map goes away", async () => {
    const { unmount } = render(<App />);
    await waitFor(() => expect(StubEventSource.last).toBeDefined());

    unmount();

    expect(StubEventSource.last?.closed).toBe(true);
  });

  it("says it is waiting rather than showing an empty map as a finished one", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("{}", { status: 503 })),
    );
    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Waiting for the first positions"),
    );
  });
});

describe("the line colour", () => {
  it("is the same for two buses on one line and different across lines", () => {
    expect(lineColor("550")).toBe(lineColor("550"));
    expect(lineColor("550")).not.toBe(lineColor("551"));
  });

  it("is always one of the design tokens' chart colours, never a value of the entity (AP-123)", () => {
    for (const line of ["550", "23", "1000N", "</style>", "red;background:url(x)"]) {
      expect(DEFAULT_TOKENS.chart.palette).toContain(lineColor(line));
    }
  });

  it("never gives a line the palette's grey, which reads as no line", () => {
    const lines = Array.from({ length: 200 }, (_, index) => `${index}`);
    expect(lines.map((line) => lineColor(line))).not.toContain("#4b5563");
  });

  it("falls back to the tokens' muted colour when the policy hides the line", () => {
    expect(lineColor(undefined)).toBe(DEFAULT_TOKENS.color.muted);
    expect(lineColor("")).toBe(DEFAULT_TOKENS.color.muted);
  });
});

describe("the charts' numbers", () => {
  const fleet: Vehicle[] = [
    { id: "a", coordinates: [24.94, 60.17], speed: 0, refLine: "550" },
    { id: "b", coordinates: [24.95, 60.17], speed: 8.5, refLine: "550" },
    { id: "c", coordinates: [24.96, 60.18], speed: 20, refLine: "23" },
    { id: "d", coordinates: [25.2, 60.3], refLine: "71" },
    { id: "e", coordinates: [24.9, 60.2], speed: 5 },
  ];

  it("counts buses per line, most first, a hidden line under its own name", () => {
    const bars = perLine(fleet);
    expect(bars.map((bar) => [bar.label, bar.value])).toEqual([["550", 2], ["23", 1], ["71", 1], [NO_LINE, 1]]);
    expect(bars[0].color).toBe(lineColor("550"));
    expect(bars[3].color).toBe(DEFAULT_TOKENS.color.muted);
  });

  it("keeps the top lines and sums the rest", () => {
    const many: Vehicle[] = Array.from({ length: 20 }, (_, index) => ({ id: `${index}`, coordinates: [24.9, 60.1], refLine: `${index + 1}` }));
    const bars = perLine(many, 15);
    expect(bars).toHaveLength(16);
    expect(bars[15]).toMatchObject({ label: "Other lines", value: 5 });
    expect(bars.reduce((sum, bar) => sum + bar.value, 0)).toBe(20);
  });

  it("bands the speed in km/h and counts a missing reading apart, never as standing still", () => {
    const { bars, unknown } = speedBands(fleet);
    expect(unknown).toBe(1);
    // 0 and 5 m/s (18 km/h) sit low, 8.5 m/s is 30.6 km/h, 20 m/s is 72 km/h.
    expect(Object.fromEntries(bars.map((bar) => [bar.label, bar.value]))).toEqual({
      "0–10 km/h": 1,
      "10–20 km/h": 1,
      "20–30 km/h": 0,
      "30–40 km/h": 1,
      "40–50 km/h": 0,
      "50–60 km/h": 0,
      "60+ km/h": 1,
    });
    expect(bars[0].color).toBe(DEFAULT_TOKENS.map.low);
    expect(bars[bars.length - 1].color).toBe(DEFAULT_TOKENS.color.danger);
    expect(new Set(bars.map((bar) => bar.color)).size).toBe(bars.length);
    expect(speedBands([]).bars.every((bar) => bar.value === 0)).toBe(true);
    expect(speedBands([{ id: "x", coordinates: [0, 0], speed: -1 }]).unknown).toBe(1);
  });

  it("keeps what the map shows and everything before the map says where it looks", () => {
    expect(inView(fleet, undefined)).toHaveLength(5);
    expect(inView(fleet, [[24.93, 60.16], [24.955, 60.175]]).map((vehicle) => vehicle.id)).toEqual(["a", "b"]);
    expect(inView([], [[0, 0], [1, 1]])).toEqual([]);
  });

  it("draws an arrow of the size it says, opaque at the tip and clear in the corners", () => {
    const { width, height, data } = arrowImage(24);
    expect([width, height, data.length]).toEqual([24, 24, 24 * 24 * 4]);
    const alpha = (x: number, y: number) => data[(y * 24 + x) * 4 + 3];
    expect(alpha(12, 6)).toBe(255);
    expect(alpha(0, 0)).toBe(0);
    expect(alpha(23, 23)).toBe(0);
  });
});

describe("the charts on the page", () => {
  it("draw one bar per line and every speed band, coloured, and follow the map's view", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify([bus(BUS, 24.94), bus(OTHER, 24.8, "551"), { ...bus("urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-03", 24.95), speed: 20 }]), { status: 200 }),
      ),
    );
    render(<App />);
    load?.();

    const perLineChart = await screen.findByRole("list", { name: "Vehicles per line" });
    await waitFor(() => expect(perLineChart.querySelectorAll("li")).toHaveLength(2));
    expect(perLineChart).toHaveTextContent("550");
    expect(perLineChart).toHaveTextContent("551");
    const speed = screen.getByRole("list", { name: "Speed" });
    expect(speed.querySelectorAll("li")).toHaveLength(7);
    for (const fill of screen.getAllByTestId("bar-fill")) {
      expect(fill.style.background).not.toBe("");
    }
    expect(screen.getByRole("list", { name: "Lines on the map" })).toHaveTextContent("550");

    // Panned to the east of 24.9: the bus on the 551 leaves the charts and the legend.
    view = [[24.9, 60.1], [25.0, 60.3]];
    moveend?.();
    await waitFor(() => expect(screen.getByRole("list", { name: "Vehicles per line" }).querySelectorAll("li")).toHaveLength(1));
    expect(screen.getByRole("list", { name: "Lines on the map" })).not.toHaveTextContent("551");
    expect(screen.getByRole("status")).toHaveTextContent("3 vehicles, 2 in view");
  });

  it("says so when no bus is in view instead of drawing empty bars", async () => {
    render(<App />);
    load?.();
    await screen.findByRole("list", { name: "Vehicles per line" });
    view = [[10, 50], [10.1, 50.1]];
    moveend?.();
    await waitFor(() => expect(screen.getAllByText("No vehicles in view.")).toHaveLength(2));
  });
});

describe("the basemap (AP-67)", () => {
  const STYLE = "https://portal.example/api/v1/projects/helsinki/basemap/default/style.json";
  function page(config: string) {
    const script = document.createElement("script");
    script.id = "jc-config";
    script.type = "application/json";
    script.textContent = config;
    document.head.append(script);
  }

  it("is the project's, from the page the App's server wrote, and no tile host of the App's own", async () => {
    page(JSON.stringify({ slug: "s", basemap: STYLE }));
    render(<App />);
    expect(constructed?.style).toBe(STYLE);
    expect(JSON.stringify(constructed?.style)).not.toContain("openstreetmap");
    expect(screen.queryByText("No basemap is configured")).not.toBeInTheDocument();
  });

  it("is the SDK's plain background, and says so, when the installation configures none", () => {
    render(<App />);
    expect(typeof constructed?.style).toBe("object");
    expect(JSON.stringify(constructed?.style)).not.toContain("openstreetmap");
    expect(screen.getByText("No basemap is configured")).toBeInTheDocument();
  });

  it("reads only an https style URL out of a well-formed configuration", () => {
    expect(basemapOf()).toBeUndefined();
    page("{not json");
    expect(basemapOf()).toBeUndefined();
    document.getElementById("jc-config")?.remove();
    page(JSON.stringify({ basemap: "javascript:alert(1)" }));
    expect(basemapOf()).toBeUndefined();
    document.getElementById("jc-config")?.remove();
    page(JSON.stringify({ basemap: STYLE }));
    expect(basemapOf()).toBe(STYLE);
  });
});
