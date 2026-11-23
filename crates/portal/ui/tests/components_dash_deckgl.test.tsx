import { render } from "@testing-library/react";
import type { Map as MapLibreMap } from "maplibre-gl";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Feature } from "geojson";
import { DeckGlOverlay, deckLayerFor } from "../src/components/dashboards/DeckGlOverlay";
import type { DenseLayer } from "../src/components/dashboards/DeckGlOverlay";
import { plainColour, RAMP, rgbOf } from "../src/components/dashboards/mapColours";

// deck.gl needs WebGL: each layer class records the props it was built with, which is what the
// overlay hands to the GPU.
type Props = Record<string, unknown>;
const built = vi.hoisted(() => ({
  list: [] as { kind: string; props: Record<string, unknown> }[],
  overlays: [] as { props: Record<string, unknown>; setProps: (props: Record<string, unknown>) => void }[],
  onReady: undefined as ((map: unknown) => (() => void) | void) | undefined,
}));

vi.mock("@deck.gl/aggregation-layers", () => ({
  HexagonLayer: class {
    constructor(props: Props) {
      built.list.push({ kind: "hexagon", props });
    }
  },
  HeatmapLayer: class {
    constructor(props: Props) {
      built.list.push({ kind: "heatmap", props });
    }
  },
}));
vi.mock("@deck.gl/layers", () => ({
  ScatterplotLayer: class {
    constructor(props: Props) {
      built.list.push({ kind: "scatter", props });
    }
  },
  GeoJsonLayer: class {
    constructor(props: Props) {
      built.list.push({ kind: "geojson", props });
    }
  },
}));
vi.mock("@deck.gl/mapbox", () => ({
  MapboxOverlay: class {
    props: Record<string, unknown>;
    setProps = vi.fn();
    constructor(props: Record<string, unknown>) {
      this.props = props;
      built.overlays.push(this);
    }
  },
}));
vi.mock("../src/components/dashboards/MapLibreView", () => ({
  MapLibreView: ({ onReady }: { onReady: (map: unknown) => (() => void) | void }) => {
    built.onReady = onReady;
    return null;
  },
}));

const point = (coordinates: unknown, properties: Record<string, unknown> | null = {}): Feature =>
  ({ type: "Feature", properties, geometry: { type: "Point", coordinates } }) as Feature;
const line: Feature = { type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: [[0, 0], [1, 1]] } };

const dense = (over: Partial<DenseLayer>): DenseLayer => ({ name: "d", url: "/q", style: "circle", features: [], ...over });

type Accessor<T> = (feature: Feature) => T;
const accessor = <T,>(name: string): Accessor<T> => built.list[0].props[name] as Accessor<T>;

describe("deckLayerFor", () => {
  beforeEach(() => {
    built.list.length = 0;
    built.overlays.length = 0;
  });

  it("hands a hexagon layer only the points it can place, at their coordinates", () => {
    const good = point([17.1, 48.1]);
    deckLayerFor(dense({ style: "hexagon", features: [good, line, point(null), point(["x", 1]), point([1])] }));
    const { props } = built.list[0];
    expect(built.list[0].kind).toBe("hexagon");
    expect(props.data).toEqual([good]);
    expect(accessor<number[]>("getPosition")(good)).toEqual([17.1, 48.1]);
    expect(accessor<number[]>("getPosition")(line)).toEqual([0, 0]);
  });

  it("weights a heatmap by the scaled value of the colour property", () => {
    deckLayerFor(dense({ style: "heatmap", colorBy: { property: "pm10", domain: [0, 200] }, features: [point([1, 2], { pm10: 50 })] }));
    expect(built.list[0].kind).toBe("heatmap");
    const weight = accessor<number>("getWeight");
    expect(weight(point([1, 2], { pm10: 50 }))).toBe(0.25);
    expect(weight(point([1, 2], { pm10: 900 }))).toBe(1);
    expect(weight(point([1, 2], { pm10: -5 }))).toBe(0);
    expect(accessor<number[]>("getPosition")(point([3, 4]))).toEqual([3, 4]);
  });

  it("scales to zero where nothing is encoded, the value is not a number or the domain is flat", () => {
    deckLayerFor(dense({ style: "heatmap" }));
    expect(accessor<number>("getWeight")(point([1, 2], { pm10: 5 }))).toBe(0);
    built.list.length = 0;
    deckLayerFor(dense({ style: "heatmap", colorBy: { property: "pm10", domain: [10, 10] } }));
    expect(accessor<number>("getWeight")(point([1, 2], { pm10: 5 }))).toBe(0);
    built.list.length = 0;
    deckLayerFor(dense({ style: "heatmap", colorBy: { property: "pm10" } }));
    const weight = accessor<number>("getWeight");
    expect(weight(point([1, 2], { pm10: "n/a" }))).toBe(0);
    expect(weight(point([1, 2], null))).toBe(0);
    expect(weight(point([1, 2], { pm10: 40 }))).toBe(0.4);
  });

  it("colours and sizes circles from the ramp and the size range", () => {
    deckLayerFor(
      dense({ style: "circle", colorBy: { property: "pm10", domain: [0, 100] }, sizeBy: { property: "pm10", range: [4, 12] }, features: [point([1, 2])] }),
    );
    const { props } = built.list[0];
    expect(built.list[0].kind).toBe("scatter");
    expect(props.radiusUnits).toBe("pixels");
    const colour = accessor<number[]>("getFillColor");
    expect(colour(point([1, 2], { pm10: 0 }))).toEqual(rgbOf(RAMP[0]));
    expect(colour(point([1, 2], { pm10: 100 }))).toEqual(rgbOf(RAMP[3]));
    const radius = accessor<number>("getRadius");
    expect(radius(point([1, 2], { pm10: 50 }))).toBe(8);
    expect(radius(point([1, 2], { pm10: 100 }))).toBe(12);
  });

  it("paints circles in the plain colour and a fixed radius when no value is encoded", () => {
    deckLayerFor(dense({ style: "circle", features: [point([1, 2])] }));
    expect(accessor<number[]>("getFillColor")(point([1, 2]))).toEqual(rgbOf(plainColour()));
    expect(accessor<number>("getRadius")(point([1, 2]))).toBe(5);
    expect(accessor<number[]>("getPosition")(point([5, 6]))).toEqual([5, 6]);
    expect(accessor<number[]>("getPosition")(line)).toEqual([0, 0]);
  });

  it("draws lines and polygons with every feature, filled only for a fill style", () => {
    const features = [line, point([1, 2])];
    deckLayerFor(dense({ style: "fill", colorBy: { property: "pm10" }, features }));
    const filled = built.list[0].props;
    expect(built.list[0].kind).toBe("geojson");
    expect(filled.data).toEqual(features);
    expect(filled.filled).toBe(true);
    expect(accessor<number[]>("getFillColor")(point([1, 2], { pm10: 100 }))).toEqual(rgbOf(RAMP[3]));
    expect(accessor<number[]>("getLineColor")(point([1, 2], { pm10: 0 }))).toEqual(rgbOf(RAMP[0]));
    built.list.length = 0;
    deckLayerFor(dense({ style: "line", features }));
    expect(built.list[0].props.filled).toBe(false);
  });
});

describe("DeckGlOverlay", () => {
  beforeEach(() => {
    built.list.length = 0;
    built.overlays.length = 0;
    built.onReady = undefined;
  });

  it("adds one overlay control to the map, feeds it new dense layers and removes it on cleanup", () => {
    const first = [dense({ name: "one", features: [point([1, 2])] })];
    const { rerender } = render(<DeckGlOverlay layers={[]} dense={first} label="Map" />);
    const map = { addControl: vi.fn(), removeControl: vi.fn() };
    const detach = built.onReady?.(map as unknown as MapLibreMap);
    expect(built.overlays).toHaveLength(1);
    expect(built.overlays[0].props.layers).toHaveLength(1);
    expect(map.addControl).toHaveBeenCalledWith(built.overlays[0]);

    rerender(<DeckGlOverlay layers={[]} dense={first} label="Map" />);
    expect(built.overlays[0].setProps).not.toHaveBeenCalled();

    rerender(<DeckGlOverlay layers={[]} dense={[...first, dense({ name: "two" })]} label="Map" />);
    expect(built.overlays[0].setProps).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(built.overlays[0].setProps).mock.calls[0][0] as { layers: unknown[] };
    expect(sent.layers).toHaveLength(2);

    if (typeof detach === "function") detach();
    expect(map.removeControl).toHaveBeenCalledWith(built.overlays[0]);
    rerender(<DeckGlOverlay layers={[]} dense={[]} label="Map" />);
    expect(built.overlays[0].setProps).toHaveBeenCalledTimes(1);
  });
});
