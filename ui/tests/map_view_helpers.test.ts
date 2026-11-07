/**
 * UI-15, UI-20, AP-67 (T-2137): the three decisions `MapLibreView` makes before it draws.
 *
 * jsdom has no WebGL, so the view itself is exercised through the App in `deckgl_overlay` and
 * the dashboard page tests. What is here is what a map decides without a canvas: where its
 * style comes from — the Portal's own origin and nowhere else, because `connect-src 'self'`
 * refuses a third-party style and a page with no project must still draw ground — and the
 * extent it opens on, which is read from the data and never guessed.
 */
import { describe, expect, it, vi } from "vitest";

// The module pulls in maplibre-gl at import time; the map itself is not what these cases are.
vi.mock("maplibre-gl", () => ({
  default: { Map: class {}, NavigationControl: class {}, Popup: class {} },
  Map: class {},
  NavigationControl: class {},
  Popup: class {},
  setWorkerUrl: () => undefined,
}));

const { blankStyle, extentOf, mapStyleFor } = await import("../src/components/dashboards/MapLibreView");

describe("where a map's style comes from", () => {
  it("asks the Portal's own origin for the project's basemap", () => {
    const style = mapStyleFor("helsinki");
    expect(typeof style).toBe("string");
    expect(style).toBe(`${window.location.origin}/api/v1/projects/helsinki/basemap/default/style.json`);
  });

  it("escapes a project whose name would otherwise change the path", () => {
    expect(mapStyleFor("a/b?c")).toContain("/projects/a%2Fb%3Fc/basemap/");
  });

  it("draws ground rather than nothing when there is no project", () => {
    const style = blankStyle();
    expect(mapStyleFor(undefined)).toEqual(style);
    expect(style.sources, "a blank style fetches nothing").toEqual({});
    expect(style.layers).toHaveLength(1);
    expect(style.layers[0].type).toBe("background");
  });
});

describe("the extent a map opens on", () => {
  const collection = (coordinates: unknown) => ({
    type: "FeatureCollection",
    features: [{ type: "Feature", geometry: { coordinates } }],
  });

  it("covers every coordinate of a collection, however deep the geometry nests", () => {
    expect(
      extentOf({
        type: "FeatureCollection",
        features: [
          { geometry: { coordinates: [19.1, 48.7] } },
          { geometry: { coordinates: [[[19.3, 48.9], [19.0, 48.6]]] } },
        ],
      }),
    ).toEqual([19.0, 48.6, 19.3, 48.9]);
  });

  it("is nothing at all when there is nothing to open on", () => {
    expect(extentOf(undefined)).toBeNull();
    expect(extentOf({})).toBeNull();
    expect(extentOf({ features: [] })).toBeNull();
    expect(extentOf(collection([]))).toBeNull();
  });

  it("ignores a coordinate that is not a pair of numbers", () => {
    // A feature whose coordinate arrived as a string, or as NaN from a division: the map opens
    // on the rest rather than on the whole world.
    expect(extentOf(collection(["19.1", "48.7"]))).toBeNull();
    expect(extentOf(collection([Number.NaN, 48.7]))).toBeNull();
    expect(
      extentOf({
        type: "FeatureCollection",
        features: [{ geometry: { coordinates: [19.1, 48.7] } }, { geometry: {} }],
      }),
    ).toEqual([19.1, 48.7, 19.1, 48.7]);
  });
});
