import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_TOKENS, extent, JcProvider, NO_BASEMAP, NO_LOCATIONS } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

interface MapOptions {
  container: HTMLElement;
  style: unknown;
  center: [number, number];
  zoom: number;
}

const mapInstances: any[] = [];
let loadHandler: (() => void) | undefined;
let errorHandler: ((event: { error?: { message?: string } }) => void) | undefined;
const clickHandlers: Record<string, (e: any) => void> = {};

vi.mock("maplibre-gl", () => {
  class Map {
    setData = vi.fn();
    fitBounds = vi.fn();
    addControl = vi.fn();
    removeControl = vi.fn();
    setPaintProperty = vi.fn();
    addSource = vi.fn();
    addLayer = vi.fn();
    easeTo = vi.fn();
    expansion = vi.fn(async (_id: number) => 14);
    getSource = vi.fn(() => ({ setData: this.setData, getClusterExpansionZoom: this.expansion }));
    remove = vi.fn();

    constructor(public options: MapOptions) {
      mapInstances.push(this);
    }

    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") {
        loadHandler = args[0] as () => void;
      } else if (event === "error" && typeof args[0] === "function") {
        errorHandler = args[0] as typeof errorHandler;
      } else if (event === "click" && typeof args[0] === "string" && typeof args[1] === "function") {
        clickHandlers[args[0]] = args[1] as (e: any) => void;
      }
    }
  }
  return { Map, setWorkerUrl: vi.fn() };
});

const mockOverlayInstances: any[] = [];
vi.mock("@deck.gl/mapbox", () => {
  class MapboxOverlay {
    props: any;
    setProps = vi.fn((p) => {
      this.props = p;
    });
    constructor(props: any) {
      this.props = props;
      mockOverlayInstances.push(this);
    }
  }
  return { MapboxOverlay };
});

vi.mock("@deck.gl/layers", () => {
  class ScatterplotLayer {
    constructor(public props: any) {}
  }
  return { ScatterplotLayer };
});

vi.mock("@deck.gl/aggregation-layers", () => {
  class HexagonLayer {
    constructor(public props: any) {}
  }
  class GridLayer {
    constructor(public props: any) {}
  }
  return { HexagonLayer, GridLayer };
});

import { GridLayer, HexagonLayer } from "@deck.gl/aggregation-layers";
import { ScatterplotLayer } from "@deck.gl/layers";
import { applyTokens } from "@joinedcontext/sdk";
import { colorRamp, EntityMap, renderPath } from "./EntityMap";

const STATIONS: Row[] = [
  {
    id: "urn:ngsi-ld:Station:001",
    type: "Station",
    name: "Kaivopuisto",
    bikes: 10,
    location: { type: "Point", coordinates: [24.95, 60.155] },
  },
  {
    id: "urn:ngsi-ld:Station:002",
    type: "Station",
    name: "Kamppi",
    bikes: 2,
    location: { type: "Point", coordinates: [24.93, 60.17] },
  },
  {
    id: "urn:ngsi-ld:Station:003",
    type: "Station",
    name: "No Location",
    bikes: 0,
    location: null,
  },
];

describe("EntityMap component and helpers", () => {
  beforeEach(() => {
    mapInstances.length = 0;
    mockOverlayInstances.length = 0;
    loadHandler = undefined;
    for (const k of Object.keys(clickHandlers)) delete clickHandlers[k];
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renderPath thresholds (49 999 / 50 000, explicit modes)", () => {
    expect(renderPath(49_999, "auto")).toBe("maplibre");
    expect(renderPath(50_000, "auto")).toBe("deck-points");
    expect(renderPath(100_000, "auto")).toBe("deck-points");

    expect(renderPath(10, "hexbin")).toBe("deck-hexbin");
    expect(renderPath(60_000, "hexbin")).toBe("deck-hexbin");
    expect(renderPath(10, "grid")).toBe("deck-grid");
    expect(renderPath(60_000, "grid")).toBe("deck-grid");
    expect(renderPath(100_000, "points")).toBe("maplibre");
  });

  it("colorRamp ends and fallback", () => {
    const tokens = DEFAULT_TOKENS;
    // Low end
    expect(colorRamp(0, [0, 100], tokens)).toBe(tokens.map.low);
    // High end
    expect(colorRamp(100, [0, 100], tokens)).toBe(tokens.map.high);
    // Midpoint
    const mid = colorRamp(50, [0, 100], tokens);
    expect(mid.startsWith("#")).toBe(true);
    expect(mid).not.toBe(tokens.map.low);
    expect(mid).not.toBe(tokens.map.high);

    // Fallbacks
    expect(colorRamp(null, [0, 100], tokens)).toBe(tokens.map.point);
    expect(colorRamp("text", [0, 100], tokens)).toBe(tokens.map.point);
    expect(colorRamp(10, null, tokens)).toBe(tokens.map.point);
    expect(colorRamp(10, [50, 50], tokens)).toBe(tokens.map.point);
  });

  it("maplibre path sets data with only rows that have geometry after load, click selects row", async () => {
    const client = stubClient({ entities: STATIONS });
    const onSelect = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityMap rows={STATIONS} location="location" label="name" onSelect={onSelect} />
      </JcProvider>,
    );

    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const map = mapInstances[0];

    loadHandler?.();

    await waitFor(() => expect(map.setData).toHaveBeenCalled());
    const passedData = map.setData.mock.calls[0][0];
    expect(passedData.features).toHaveLength(2);
    expect(passedData.features.map((f: any) => f.id)).toEqual(["urn:ngsi-ld:Station:001", "urn:ngsi-ld:Station:002"]);

    expect(screen.getByText("2 on the map")).toBeInTheDocument();
    expect(screen.getByText(NO_BASEMAP)).toBeInTheDocument();

    clickHandlers["jc-points"]?.({ features: [{ properties: { id: "urn:ngsi-ld:Station:002" } }] });
    expect(onSelect).toHaveBeenCalledWith(STATIONS[1]);
  });

  it("says the data has no locations instead of 0 on the map, and labels rows by name, never by URN", async () => {
    const client = stubClient({ entities: STATIONS });
    const unplaced = STATIONS.map((row) => ({ ...row, location: null }));
    const { unmount } = render(
      <JcProvider client={client}>
        <EntityMap rows={unplaced} location="location" />
      </JcProvider>,
    );
    expect(screen.getByText(NO_LOCATIONS)).toBeInTheDocument();
    expect(screen.queryByText(/0 on the map/)).not.toBeInTheDocument();
    unmount();

    render(
      <JcProvider client={client}>
        <EntityMap rows={[]} location="location" />
      </JcProvider>,
    );
    expect(screen.queryByText(NO_LOCATIONS)).not.toBeInTheDocument();
  });

  it("colours by the data's own range: equal values and missing ones stay the plain point colour", () => {
    const tokens = DEFAULT_TOKENS;
    const rows: Row[] = [
      { id: "a", type: "S", bikes: 0 },
      { id: "b", type: "S", bikes: 0 },
      { id: "c", type: "S", bikes: null },
    ];
    const range = extent(rows, "bikes");
    expect(colorRamp(rows[0].bikes, range, tokens)).toBe(tokens.map.point);
    expect(colorRamp(rows[2].bikes, range, tokens)).toBe(tokens.map.point);
    const varied = extent([...rows, { id: "d", type: "S", bikes: 12 }], "bikes");
    expect(colorRamp(0, varied, tokens)).toBe(tokens.map.low);
    expect(colorRamp(12, varied, tokens)).toBe(tokens.map.high);
  });

  it("mode='hexbin' adds an overlay whose layers hold one HexagonLayer with point rows", async () => {
    const client = stubClient({ entities: STATIONS });

    render(
      <JcProvider client={client}>
        <EntityMap
          rows={STATIONS}
          location="location"
          label="name"
          mode="hexbin"
          basemap="https://example.com/style.json"
          selected="urn:ngsi-ld:Station:001"
        />
      </JcProvider>,
    );

    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const map = mapInstances[0];

    loadHandler?.();

    await waitFor(() => expect(map.addControl).toHaveBeenCalled());
    expect(mockOverlayInstances).toHaveLength(1);

    const overlay = mockOverlayInstances[0];
    expect(overlay.props.layers).toHaveLength(1);
    expect(overlay.props.layers[0]).toBeInstanceOf(HexagonLayer);
    expect(overlay.props.layers[0].props.data).toHaveLength(2);

    expect(screen.queryByText(NO_BASEMAP)).not.toBeInTheDocument();
    expect(screen.getByText("2 on the map · Kaivopuisto")).toBeInTheDocument();
  });

  // The paths a big or an aggregated map takes (T-3395): deck.gl's layers over MapLibre, each
  // reading the rows' positions and colours, a dot clicked selecting its row.
  it("draws fifty thousand rows as deck.gl points, coloured and clickable", async () => {
    // A short and an odd hex colour reach deck.gl as numbers all the same.
    applyTokens({ ...DEFAULT_TOKENS, map: { ...DEFAULT_TOKENS.map, point: "#abc", low: "#abcd", high: "#ff0000" } });
    const many: Row[] = Array.from({ length: 50_000 }, (_, i) => ({ id: `urn:x:${i}`, type: "S", bikes: i % 7, location: { type: "Point", coordinates: [24.9, 60.1] } }));
    many.push({ id: "urn:x:line", type: "S", location: { type: "MultiLineString", coordinates: [[[24.8, 60.2], [24.81, 60.21]]] } });
    const onSelect = vi.fn();
    render(
      <JcProvider client={stubClient({ entities: [] })}>
        <EntityMap rows={many} location="location" color="bikes" onSelect={onSelect} />
      </JcProvider>,
    );
    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    loadHandler?.();
    await waitFor(() => expect(mockOverlayInstances).toHaveLength(1));
    const layer = mockOverlayInstances[0].props.layers[0];
    expect(layer).toBeInstanceOf(ScatterplotLayer);
    const first = layer.props.data[0];
    expect(layer.props.getPosition(first)).toEqual([24.9, 60.1]);
    expect(layer.props.getFillColor(first)).toHaveLength(3);
    layer.props.onClick({});
    expect(onSelect).not.toHaveBeenCalled();
    layer.props.onClick({ object: first });
    expect(onSelect).toHaveBeenCalledWith(many[0]);
    applyTokens(DEFAULT_TOKENS);
  });

  it("aggregates into a grid, the hexagons read their positions, and the overlay goes for plain points", async () => {
    const client = stubClient({ entities: STATIONS });
    const view = (mode: "grid" | "hexbin" | "points") => (
      <JcProvider client={client}>
        <EntityMap rows={STATIONS} location="location" mode={mode} />
      </JcProvider>
    );
    const { rerender } = render(view("grid"));
    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const map = mapInstances[0];
    loadHandler?.();
    await waitFor(() => expect(mockOverlayInstances).toHaveLength(1));
    const grid = mockOverlayInstances[0].props.layers[0];
    expect(grid).toBeInstanceOf(GridLayer);
    expect(grid.props.getPosition(grid.props.data[0])).toEqual([24.95, 60.155]);
    rerender(view("hexbin"));
    await waitFor(() => expect(mockOverlayInstances[0].props.layers[0]).toBeInstanceOf(HexagonLayer));
    const hex = mockOverlayInstances[0].props.layers[0];
    expect(hex.props.getPosition(hex.props.data[1])).toEqual([24.93, 60.17]);
    rerender(view("points"));
    await waitFor(() => expect(map.removeControl).toHaveBeenCalled());
  });

  // T-2923: the events map colours each point by its register and clusters close ones; a click on a
  // cluster zooms in on it, on a point selects its row, and what is no cluster or no row does nothing.
  it("colours by the app's own palette, clusters, and zooms in on a clicked cluster only", async () => {
    const colorOf = (row: Row) => (row.id.endsWith("001") ? "#112233" : "#445566");
    const onSelect = vi.fn();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(
      <JcProvider client={stubClient({ entities: [] })}>
        <EntityMap rows={STATIONS} location="location" colorOf={colorOf} cluster onSelect={onSelect} />
      </JcProvider>,
    );
    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const map = mapInstances[0];
    loadHandler?.();
    expect(map.addSource).toHaveBeenCalledWith("jc-rows", expect.objectContaining({ cluster: true, clusterRadius: 40 }));
    expect(map.addLayer).toHaveBeenCalledWith(expect.objectContaining({ id: "jc-clusters", filter: ["has", "point_count"] }));
    await waitFor(() => expect(map.setData).toHaveBeenCalled());
    expect(map.setData.mock.calls.at(-1)[0].features.map((f: any) => f.properties.color)).toEqual(["#112233", "#445566"]);

    const zoomIn = clickHandlers["jc-clusters"];
    const at = { type: "Point", coordinates: [24.94, 60.17] };
    zoomIn({ features: [{ properties: { cluster_id: 3 }, geometry: at }] });
    await waitFor(() => expect(map.easeTo).toHaveBeenCalledWith({ center: [24.94, 60.17], zoom: 14 }));
    expect(map.expansion).toHaveBeenCalledWith(3);
    // No cluster id, no point, no feature: nothing to zoom to.
    zoomIn({ features: [{ properties: {}, geometry: at }] });
    zoomIn({ features: [{ properties: { cluster_id: 4 }, geometry: { type: "LineString", coordinates: [] } }] });
    zoomIn({});
    expect(map.expansion).toHaveBeenCalledTimes(1);
    // A source the map lost, and one that cannot say the zoom: logged, never thrown.
    map.getSource.mockReturnValueOnce(undefined);
    zoomIn({ features: [{ properties: { cluster_id: 5 }, geometry: at }] });
    map.expansion.mockRejectedValueOnce(new Error("gone"));
    zoomIn({ features: [{ properties: { cluster_id: 6 }, geometry: at }] });
    await waitFor(() => expect(error).toHaveBeenCalledWith("jc: cluster zoom", expect.any(Error)));
    expect(map.easeTo).toHaveBeenCalledTimes(1);

    clickHandlers["jc-points"]({ features: [{ properties: { id: "urn:ngsi-ld:Station:404" } }] });
    clickHandlers["jc-points"]({ features: [{ properties: {} }] });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("colours deck.gl's points by the app's own palette too", async () => {
    const many: Row[] = Array.from({ length: 50_000 }, (_, i) => ({ id: `urn:x:${i}`, type: "S", location: { type: "Point", coordinates: [24.9, 60.1] } }));
    render(
      <JcProvider client={stubClient({ entities: [] })}>
        <EntityMap rows={many} location="location" colorOf={() => "#ff0000"} />
      </JcProvider>,
    );
    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    loadHandler?.();
    await waitFor(() => expect(mockOverlayInstances).toHaveLength(1));
    const layer = mockOverlayInstances[0].props.layers[0];
    expect(layer.props.getFillColor(layer.props.data[0])).toEqual([255, 0, 0]);
  });

  it("logs what the map says went wrong", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(
      <JcProvider client={stubClient({ entities: STATIONS })}>
        <EntityMap rows={STATIONS} location="location" />
      </JcProvider>,
    );
    await waitFor(() => expect(errorHandler).toBeDefined());
    errorHandler?.({ error: { message: "tile 404" } });
    errorHandler?.({});
    expect(logged).toHaveBeenCalledWith("jc: map error", "tile 404");
    expect(logged).toHaveBeenCalledTimes(2);
  });
});
