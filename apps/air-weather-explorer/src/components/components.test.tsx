/**
 * The chart card and the map on their own: a chart redraws on resize and hands the clicked
 * category on; the map fits its points once, logs its own errors, and builds nothing for a page
 * already gone.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { Map as FakeMap } from "../testing/maplibre";
import { ChartCard } from "./ChartCard";
import { MapView } from "./MapView";

vi.mock("maplibre-gl", () => import("../testing/maplibre"));

const drawn = vi.hoisted(() => ({ click: null as ((params: unknown) => void) | null, resized: 0, disposed: 0 }));
vi.mock("echarts/core", () => ({
  use: () => undefined,
  init: () => ({
    setOption: () => undefined,
    resize: () => {
      drawn.resized += 1;
    },
    dispose: () => {
      drawn.disposed += 1;
    },
    on: (_event: string, handler: (params: unknown) => void) => {
      drawn.click = handler;
    },
  }),
}));

afterEach(() => {
  vi.unstubAllGlobals();
  FakeMap.built.length = 0;
});

describe("ChartCard", () => {
  it("redraws when its box changes, hands a clicked category on, and lets go of the chart when it leaves", () => {
    let observed: (() => void) | null = null;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe() {}
        disconnect() {}
      },
    );
    const onSelect = vi.fn();
    const { unmount } = render(<ChartCard title="Correlations" option={{ series: [] }} onSelect={onSelect} />);
    expect(screen.getByRole("img", { name: "Correlations" })).toBeInTheDocument();
    act(() => observed?.());
    expect(drawn.resized).toBe(1);
    drawn.click?.({ name: "pm10|windSpeed" });
    drawn.click?.({});
    expect(onSelect).toHaveBeenCalledExactlyOnceWith("pm10|windSpeed");
    unmount();
    expect(drawn.disposed).toBeGreaterThan(0);
  });

  it("says it is reading, and what is missing, in the page's words", () => {
    const { rerender } = render(<ChartCard title="Series" option={null} loading loadingLabel="Reading the measurements…" empty="No measurements." />);
    expect(screen.getByRole("status")).toHaveTextContent("Reading the measurements…");
    rerender(<ChartCard title="Series" option={null} empty="No measurements." />);
    expect(screen.getByText("No measurements.")).toBeInTheDocument();
  });
});

function withClient(children: React.ReactNode, basemap?: string) {
  const client = stubClient({}, { appName: "air-weather-explorer", ...(basemap ? { basemap } : {}) });
  return <JcProvider client={client}>{children}</JcProvider>;
}

describe("MapView", () => {
  it("says there is no basemap when the project has none, and shows none with one", async () => {
    const { unmount } = render(withClient(<MapView points={[]} label="Stations" />));
    expect(screen.getByRole("application", { name: "Stations" })).toBeInTheDocument();
    expect(screen.getByText(/basemap/i)).toBeInTheDocument();
    unmount();
    render(withClient(<MapView points={[]} label="Stations" />, "https://portal.example/style.json"));
    expect(screen.queryByText(/basemap/i)).toBeNull();
  });

  it("logs the map's own error, with or without its message", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(withClient(<MapView points={[]} label="Stations" />));
    const map = await waitFor(() => FakeMap.built[0] ?? Promise.reject(new Error("not built yet")));
    map.fire("error", { error: { message: "style not found" } });
    map.fire("error", { type: "error" });
    expect(logged).toHaveBeenNthCalledWith(1, "map error", "style not found");
    expect(logged).toHaveBeenNthCalledWith(2, "map error", { type: "error" });
  });

  it("builds no map for a page already gone, and draws nothing after it left", async () => {
    const { unmount } = render(withClient(<MapView points={[{ id: "a", at: [24.9, 60.2], color: "red", lines: ["A"] }]} label="Stations" />));
    unmount();
    await Promise.resolve();
    await Promise.resolve();
    expect(FakeMap.built.every((map) => map.removed || map.layers.length === 0)).toBe(true);
  });
});
