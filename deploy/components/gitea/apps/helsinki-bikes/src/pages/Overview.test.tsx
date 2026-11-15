import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_TOKENS, JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { STATIONS } from "../fixtures/stations";

// T-2924: the overview draws in colour: a chart per question and every located station on the map.
const maps: Array<{ setData: ReturnType<typeof vi.fn>; load?: () => void }> = [];
vi.mock("maplibre-gl", () => {
  class Map {
    setData = vi.fn();
    fitBounds = vi.fn();
    addControl = vi.fn();
    removeControl = vi.fn();
    setPaintProperty = vi.fn();
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = vi.fn(() => ({ setData: this.setData }));
    remove = vi.fn();
    load?: () => void;
    constructor() {
      maps.push(this);
    }
    on(event: string, handler: unknown) {
      if (event === "load" && typeof handler === "function") this.load = handler as () => void;
    }
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));

const charts: Array<{ element: HTMLElement; setOption: ReturnType<typeof vi.fn> }> = [];
vi.mock("echarts", () => ({
  init: vi.fn((element: HTMLElement) => {
    const chart = { element, setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() };
    charts.push(chart);
    return chart;
  }),
}));

import { Overview } from "./Overview";

const READ = {
  permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function overview(entities: Row[] = STATIONS) {
  render(
    <JcProvider client={stubClient({ entities, access: READ }, { appName: "helsinki-bikes" })}>
      <Overview />
    </JcProvider>,
  );
}

/** The last option the chart captioned `title` was drawn with. */
function optionOf(title: string): Record<string, any> {
  const figure = screen.getByText(title).closest("figure") as HTMLElement;
  const chart = charts.filter((c) => figure.contains(c.element)).at(-1);
  expect(chart, title).toBeDefined();
  return chart!.setOption.mock.calls.at(-1)![0];
}

describe("the overview", () => {
  beforeEach(() => {
    maps.length = 0;
    charts.length = 0;
  });

  it("colours its three numbers by their meaning", async () => {
    overview();
    const region = await screen.findByRole("region", { name: "Overview" });
    const tone = (label: string) => (within(region).getByText(label).closest(".jc-tile") as HTMLElement).dataset.tone;
    await waitFor(() => expect(within(region).getByText("15")).toBeInTheDocument());
    expect([tone("Stations"), tone("Bikes available"), tone("Stations empty")]).toEqual(["accent", "success", "danger"]);
  });

  it("charts the stations by bikes available, one coloured bar per band", async () => {
    overview();
    await waitFor(() => expect(optionOf("Stations by bikes available").series[0].data).toHaveLength(6));
    const bars = optionOf("Stations by bikes available").series[0].data;
    expect(bars.map((bar: { value: number }) => bar.value)).toEqual([2, 0, 1, 0, 1, 0]);
    expect(bars[0].itemStyle.color).toBe(DEFAULT_TOKENS.map.low.toLowerCase());
    expect(bars[5].itemStyle.color).toBe(DEFAULT_TOKENS.map.high.toLowerCase());
  });

  it("charts the fullest and the emptiest stations, one bar per station with both counts", async () => {
    overview();
    await waitFor(() => expect(optionOf("Fullest stations").yAxis.data).toHaveLength(4));
    expect(optionOf("Fullest stations").yAxis.data[0]).toBe("Kapteeninpuistikko");
    // Each chart is built on its own beat (T-2994): wait for this one too.
    await waitFor(() =>
      expect(optionOf("Emptiest stations").yAxis.data.slice(0, 2)).toEqual(["Laivasillankatu", "Sepänkatu"]),
    );
    expect(optionOf("Emptiest stations").series[0].data[0].value).toBe(0);
  });

  it("puts one feature per located station on the map, coloured by its share of docks", async () => {
    overview();
    await waitFor(() => expect(maps.length).toBeGreaterThan(0));
    const map = maps[0];
    map.load?.();
    // A draw before the stations are read carries none; wait for the one that carries them (T-2994).
    await waitFor(() => expect(map.setData.mock.calls.at(-1)?.[0].features).toHaveLength(STATIONS.length));
    const features = map.setData.mock.calls.at(-1)![0].features;
    expect(features).toHaveLength(STATIONS.length);
    const colours = new Set(features.map((f: { properties: { color: string } }) => f.properties.color));
    expect(colours.size).toBeGreaterThan(1);
    expect(screen.getByLabelText("Map colour: share of docks with a bike")).toHaveTextContent("0 %");
  });

  it("says there is nothing to rank when no station reports its docks", async () => {
    overview([{ id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:x", type: "BikeHireDockingStation", name: "X" }]);
    const fullest = (await screen.findByText("Fullest stations")).closest("figure") as HTMLElement;
    await waitFor(() => expect(within(fullest).getByText("No station reports its docks yet.")).toBeInTheDocument());
  });
});
