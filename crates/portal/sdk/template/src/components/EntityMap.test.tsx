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
let mapRefused = false;
let loadHandler: (() => void) | undefined;
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
    getSource = vi.fn(() => ({ setData: this.setData }));
    remove = vi.fn();

    constructor(public options: MapOptions) {
      if (mapRefused) throw new Error("WebGL2 is required to display this map");
      mapInstances.push(this);
    }

    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") {
        loadHandler = args[0] as () => void;
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

import { HexagonLayer } from "@deck.gl/aggregation-layers";
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

  it("says the map cannot be drawn where WebGL2 is missing, instead of an unhandled error that fails the build", async () => {
    mapRefused = true;
    const client = stubClient({ entities: STATIONS });
    render(
      <JcProvider client={client}>
        <EntityMap rows={STATIONS} location="location" />
      </JcProvider>,
    );
    expect(await screen.findByRole("status")).toHaveTextContent("WebGL2 is off");
    expect(mapInstances).toHaveLength(0);
    mapRefused = false;
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

  it("AP-138: a map coloured by a value names it and its two ends in a legend; an uncoloured or flat one shows none", () => {
    const client = stubClient({ entities: STATIONS });
    const { unmount } = render(
      <JcProvider client={client}>
        <EntityMap rows={STATIONS} location="location" color="bikes" />
      </JcProvider>,
    );
    const legend = screen.getByRole("img", { name: "bikes: 0 to 10" });
    expect(legend).toHaveTextContent("bikes");
    expect(legend).toHaveTextContent("0");
    expect(legend).toHaveTextContent("10");
    unmount();

    const { unmount: unmountPlain } = render(
      <JcProvider client={client}>
        <EntityMap rows={STATIONS} location="location" />
      </JcProvider>,
    );
    expect(screen.queryByRole("img", { name: /bikes/ })).not.toBeInTheDocument();
    unmountPlain();

    const flat = STATIONS.map((row) => ({ ...row, bikes: 3 }));
    render(
      <JcProvider client={client}>
        <EntityMap rows={flat} location="location" color="bikes" />
      </JcProvider>,
    );
    expect(screen.queryByRole("img", { name: /bikes/ })).not.toBeInTheDocument();
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
});
