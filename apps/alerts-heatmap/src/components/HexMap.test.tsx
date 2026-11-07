/**
 * The hexagon map on its own: the ramp set from the data, the view fitted once, a hexagon's popup
 * in text, a click on a place answered by the place alone, the map's own errors logged, and no
 * map built for a page already gone.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import type { Hex, Place } from "../analysis";
import { Map as FakeMap, Popup } from "../testing/maplibre";
import { HexMap } from "./HexMap";

vi.mock("maplibre-gl", () => import("../testing/maplibre"));

const HEX: Hex = { id: "h1", count: 4, lon: 24.9, lat: 60.2, ring: [[24.89, 60.19], [24.91, 60.19], [24.91, 60.21], [24.89, 60.19]] };
const PLACE: Place = { count: 4, lon: 24.9, lat: 60.2, radius: 30, ids: ["urn:ngsi-ld:Alert:a"] };

function show(props: Partial<Parameters<typeof HexMap>[0]> = {}, basemap?: string) {
  const describe = vi.fn((what: { kind: string }) => (what.kind === "hex" ? ["<b>4 alerts</b>"] : ["A place"]));
  const onPlace = vi.fn();
  const view = render(
    <JcProvider client={stubClient({}, basemap ? { basemap } : {})}>
      <HexMap hexes={[HEX]} places={[PLACE]} label="Map" describe={describe} onPlace={onPlace} {...props} />
    </JcProvider>,
  );
  return { ...view, describe, onPlace };
}

async function built() {
  return waitFor(() => {
    const [map] = FakeMap.built;
    expect(map?.sources.hexes).toBeDefined();
    return map!;
  });
}

afterEach(() => {
  FakeMap.built.length = 0;
  Popup.opened.length = 0;
  vi.restoreAllMocks();
});

describe("HexMap", () => {
  it("colours the hexagons by count up to the busiest and fits the view once", async () => {
    const { rerender } = show();
    const map = await built();
    await waitFor(() => expect(map.fitted).toHaveLength(1));
    expect(map.paint["hexes.fill-color"]).toEqual(expect.arrayContaining([4]));
    rerender(
      <JcProvider client={stubClient()}>
        <HexMap hexes={[{ ...HEX, count: 9 }]} places={[]} label="Map" describe={() => []} />
      </JcProvider>,
    );
    await waitFor(() => expect(map.paint["hexes.fill-color"]).toEqual(expect.arrayContaining([9])));
    expect(map.fitted).toHaveLength(1);
  });

  it("names a hexagon in text, and leaves a click on a place to the place", async () => {
    const { onPlace } = show();
    const map = await built();
    map.fire("click", { features: [{ properties: { index: 0 } }], lngLat: { lng: 24.9, lat: 60.2 }, point: { x: 1, y: 1 } }, "hexes");
    expect(Popup.opened).toHaveLength(1);
    expect(Popup.opened[0].content?.textContent).toBe("<b>4 alerts</b>");
    expect(Popup.opened[0].content?.querySelector("b")).toBeNull();
    map.rendered = [{ layer: "places" }];
    map.fire("click", { features: [{ properties: { index: 0 } }], lngLat: { lng: 24.9, lat: 60.2 }, point: { x: 1, y: 1 } }, "hexes");
    expect(Popup.opened).toHaveLength(1);
    // A feature the page no longer lists, or none at all, opens nothing.
    map.fire("click", { features: [{ properties: { index: 7 } }], lngLat: { lng: 0, lat: 0 } }, "places");
    map.fire("click", { features: [], lngLat: { lng: 0, lat: 0 } }, "places");
    expect(Popup.opened).toHaveLength(1);
    expect(onPlace).not.toHaveBeenCalled();
  });

  it("follows its box when the page lays out again", async () => {
    // A holder, not a `let`: TypeScript narrows a `let` that only a callback assigns to `null`.
    const observer: { resized?: () => void } = {};
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          observer.resized = callback;
        }
        observe() {}
        disconnect() {}
      },
    );
    show();
    const map = await built();
    const resize = vi.spyOn(map, "resize");
    observer.resized?.();
    expect(resize).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("logs the map's own errors and says when the project has no basemap", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    show();
    expect(screen.getByText(/basemap/i)).toBeInTheDocument();
    const map = await built();
    map.fire("error", { error: { message: "style not found" } });
    map.fire("error", { type: "error" });
    expect(logged).toHaveBeenNthCalledWith(1, "alerts-heatmap: map error", "style not found");
    expect(logged).toHaveBeenNthCalledWith(2, "alerts-heatmap: map error", { type: "error" });
  });

  it("shows no notice with a basemap, and builds nothing for a page already gone", async () => {
    const { unmount } = show({}, "https://portal.example/style.json");
    expect(screen.queryByText(/basemap/i)).toBeNull();
    unmount();
    await Promise.resolve();
    await Promise.resolve();
    expect(FakeMap.built.every((map) => map.removed || map.layers.length === 0)).toBe(true);
  });
});
