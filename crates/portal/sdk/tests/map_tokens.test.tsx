/**
 * The maps take their colours from the app's design tokens (SDK-25, AP-137, T-2779).
 *
 * Every App starts from a look of its own; a map that drew in a fixed blue on a fixed grey would
 * make two Apps look alike wherever they show a map. `GeoView` draws in the tokens' map point and
 * stroke unless a colour is passed, and the empty base map is the page's line colour.
 */
import { render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const layers: Array<{ id: string; paint: Record<string, unknown> }> = [];
let loadHandler: (() => void) | undefined;

vi.mock("maplibre-gl", () => {
  class Map {
    constructor(public options: unknown) {}
    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") {
        loadHandler = args[0] as () => void;
      }
    }
    addSource = vi.fn();
    addLayer = (layer: { id: string; paint: Record<string, unknown> }) => {
      layers.push(layer);
    };
    getSource = () => ({ setData: vi.fn() });
    fitBounds = vi.fn();
    setPaintProperty = vi.fn();
    remove = vi.fn();
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { applyTokens, DEFAULT_TOKENS } = await import("../src/sdk/tokens");
const { styleFor } = await import("../src/sdk/map");
const { fillOf, GeoView } = await import("../src/geo/GeoView");

const LOOK = {
  color: { line: "#d4d9e3" },
  map: { point: "#8a2be2", stroke: "#fdfdfd" },
};

async function drawn(accent?: string) {
  render(<GeoView value={{ type: "Point", coordinates: [24.94, 60.17] }} accent={accent} />);
  await waitFor(() => expect(loadHandler).toBeDefined());
  loadHandler?.();
  const points = layers.find((layer) => layer.id === "points");
  expect(points).toBeDefined();
  return points?.paint ?? {};
}

afterEach(() => {
  layers.length = 0;
  loadHandler = undefined;
  applyTokens(DEFAULT_TOKENS);
});

describe("maps in the app's look", () => {
  it("draws a geometry in the tokens' point and stroke colours", async () => {
    applyTokens(LOOK);
    const paint = await drawn();
    expect(paint["circle-color"]).toBe("#8a2be2");
    expect(paint["circle-stroke-color"]).toBe("#fdfdfd");
    expect(layers.find((layer) => layer.id === "areas")?.paint["fill-color"]).toBe("#8a2be2");
  });

  it("lets a colour the caller passes win over the tokens", async () => {
    applyTokens(LOOK);
    const paint = await drawn("#c2410c");
    expect(paint["circle-color"]).toBe("#c2410c");
  });

  it("paints the empty base map in the page's line colour", () => {
    applyTokens(LOOK);
    const style = styleFor(undefined) as { layers: Array<{ paint: Record<string, unknown> }> };
    expect(style.layers[0].paint["background-color"]).toBe("#d4d9e3");
    // A configured basemap is the provider's style, untouched.
    expect(styleFor("https://tiles.example/style.json")).toBe("https://tiles.example/style.json");
  });

  it("fills areas on the tokens' ramp over their value, the drawn colour where there is none (T-2933)", async () => {
    applyTokens({ map: { point: "#8a2be2", low: "#fde68a", high: "#b91c1c" } });
    render(
      <GeoView
        value={[{ type: "Feature", id: "a", geometry: { type: "Point", coordinates: [19.1, 48.7] }, properties: { value: 3 } }]}
        ramp={[1, 5]}
      />,
    );
    await waitFor(() => expect(loadHandler).toBeDefined());
    loadHandler?.();
    const fill = layers.find((layer) => layer.id === "areas")?.paint["fill-color"];
    expect(fill).toEqual([
      "case",
      ["==", ["typeof", ["get", "value"]], "number"],
      ["interpolate", ["linear"], ["get", "value"], 1, "#fde68a", 5, "#b91c1c"],
      "#8a2be2",
    ]);
    // One value alone still gives rising stops.
    expect(fillOf("#000", [2, 2], { low: "#fff", high: "#111" })).toEqual([
      "case",
      ["==", ["typeof", ["get", "value"]], "number"],
      ["interpolate", ["linear"], ["get", "value"], 2, "#fff", 3, "#111"],
      "#000",
    ]);
  });
});

