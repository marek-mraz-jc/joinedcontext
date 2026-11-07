import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { Cell, StopOut } from "../analysis";
import { Map as FakeMap, Popup } from "../testing/maplibre";

vi.mock("maplibre-gl", () => import("../testing/maplibre"));
const { ReachMap } = await import("./ReachMap");

const STOPS: StopOut[] = [
  { lon: 24.94, lat: 60.17, name: "Rautatientori", code: "H0019", vehicles: 0, routes: ["M1"], minutes: 0 },
  { lon: 25.1, lat: 60.3, name: null, code: null, vehicles: 2, routes: [], minutes: null },
];
const CELLS: Cell[] = [
  { band: 10, ring: [[24.9, 60.1], [24.95, 60.1], [24.95, 60.15], [24.9, 60.1]] },
  { band: 30, ring: [[25.0, 60.2], [25.05, 60.2], [25.05, 60.25], [25.0, 60.2]] },
] as Cell[];

function draw(props: Partial<Parameters<typeof ReachMap>[0]> = {}, basemap?: string) {
  const onPick = vi.fn();
  const client = stubClient(undefined, { appName: "transit-reach", ...(basemap ? { basemap } : {}) });
  const view = render(
    <JcProvider client={client}>
      <ReachMap
        cells={CELLS}
        stops={STOPS}
        origin={{ lon: 24.94, lat: 60.17 }}
        bands={[10, 20, 30, 40]}
        label="Map"
        describe={(stop, index) => [stop.name ?? `Stop ${index + 1}`, String(stop.minutes)]}
        onPick={onPick}
        {...props}
      />
    </JcProvider>,
  );
  return { onPick, view };
}

const built = () => FakeMap.built.at(-1) as FakeMap;

afterEach(() => {
  FakeMap.built.length = 0;
  Popup.opened.length = 0;
  vi.restoreAllMocks();
});

describe("the reach map", () => {
  it("draws the cells, the stops and the start, each band in its colour, a fourth band in the far one", async () => {
    draw();
    await waitFor(() => expect(built()?.sources.cells?.data).toMatchObject({ features: [{}, {}] }));
    const map = built();
    expect((map.sources.stops.data as { features: unknown[] }).features).toHaveLength(2);
    expect(map.sources.origin.data).toMatchObject({ geometry: { coordinates: [24.94, 60.17] } });
    const [, , step] = map.paints.find(([layer]) => layer === "cells") ?? [];
    expect((step as unknown[]).length).toBe(3 + 2 * 3);
    expect(map.fitted).toEqual([[[24.9, 60.1], [25.05, 60.25]]]);
    expect(screen.getByText(/basemap/i)).toBeInTheDocument();
  });

  it("goes to the start when nothing is reached, and says nothing of a basemap it has", async () => {
    draw({ cells: [] }, "https://tiles.example.org/style.json");
    await waitFor(() => expect(built()?.eased).toEqual([{ center: [24.94, 60.17], duration: 0 }]));
    expect(screen.queryByText(/basemap/i)).toBeNull();
  });

  it("names a clicked stop in a popup of text, and starts from anywhere else", async () => {
    const { onPick } = draw();
    await waitFor(() => expect(built()?.sources.cells).toBeDefined());
    const map = built();
    map.under = [{ properties: { index: 1 } }];
    act(() => map.fire("click", { point: { x: 1, y: 1 }, lngLat: { lng: 25.1, lat: 60.3 } }));
    expect(Popup.opened).toHaveLength(1);
    expect(Popup.opened[0].content?.textContent).toBe("Stop 2null");
    expect(onPick).not.toHaveBeenCalled();
    map.under = [{ properties: { index: 9 } }];
    act(() => map.fire("click", { point: { x: 2, y: 2 }, lngLat: { lng: 24.99, lat: 60.21 } }));
    map.under = [];
    act(() => map.fire("click", { point: { x: 3, y: 3 }, lngLat: { lng: 25.0, lat: 60.22 } }));
    expect(onPick.mock.calls).toEqual([[{ lon: 24.99, lat: 60.21 }], [{ lon: 25.0, lat: 60.22 }]]);
  });

  it("says a map error in the console and lets go of everything when it is gone", async () => {
    const said = vi.spyOn(console, "error").mockImplementation(() => {});
    const { view } = draw();
    await waitFor(() => expect(built()?.sources.cells).toBeDefined());
    built().fire("error", { error: { message: "tiles refused" } });
    built().fire("error", { status: 500 });
    expect(said).toHaveBeenCalledWith("transit-reach: map error", "tiles refused");
    expect(said).toHaveBeenCalledTimes(2);
    const map = built();
    view.unmount();
    expect(map.removed).toBe(true);
    // A load that comes after the page is gone draws nothing.
    map.fire("load", {});
  });
});
