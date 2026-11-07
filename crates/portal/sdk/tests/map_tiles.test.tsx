/**
 * Maps past a count draw the Endpoint's OGC vector tiles (T-5195, T-5189, EP-29).
 *
 * MapLibre is stubbed (no WebGL in jsdom); the browser leg is `e2e/map_tiles.spec.ts`. Checked
 * here: which tile template is accepted, that a tile is read only from the page's own host under
 * the Endpoint, and when a grid's map switches to tiles.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const added: { sources: [string, Record<string, unknown>][]; layers: [Record<string, unknown>, string | undefined][] } = { sources: [], layers: [] };
const clicks: Record<string, (event: { features?: { properties?: { id?: string } }[] }) => void> = {};
const protocols: string[] = [];

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") queueMicrotask(args[0] as () => void);
      if (event === "click" && typeof args[1] === "function") clicks[args[0] as string] = args[1] as (typeof clicks)[string];
    }
    addSource = (id: string, spec: Record<string, unknown>) => added.sources.push([id, spec]);
    addLayer = (layer: Record<string, unknown>, before?: string) => added.layers.push([layer, before]);
    getSource = () => ({ setData: () => {} });
    getLayer = () => undefined;
    removeLayer = () => {};
    removeSource = () => {};
    fitBounds = () => {};
    setPaintProperty = () => {};
    remove = () => {};
  }
  return { Map, setWorkerUrl: vi.fn(), addProtocol: (name: string) => protocols.push(name) };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { tileTemplate, tilesetPath, loadTile, MVT, TILE_THRESHOLD } = await import("../src/geo/tiles");
const { GeoView, TILED_NOTE } = await import("../src/geo/GeoView");
const { EntityGrid } = await import("../src/grid/EntityGrid");
const { parseGridConfig } = await import("../src/grid/config");
const { fixtureSource, endpointSource } = await import("../src/grid/source");
type EntitySource = import("../src/grid/source").EntitySource;

const PATH = "/api/endpoint/air/ogc/features/collections/AirQualityObserved/tiles/WebMercatorQuad";
const item = (href: string, type = MVT) => ({ links: [{ rel: "self", type: "application/json", href: PATH }, { rel: "item", type, templated: true, href }] });

afterEach(() => {
  cleanup();
  added.sources.length = 0;
  added.layers.length = 0;
  vi.unstubAllGlobals();
});

describe("the tile template a tileset names", () => {
  it("is read on the page's own host when the item sits on the tileset's own path", () => {
    expect(tilesetPath("/api/endpoint/air", "AirQualityObserved")).toBe(PATH);
    const own = `${PATH}/{z}/{y}/{x}`;
    expect(tileTemplate(item(`${PATH}/{tileMatrix}/{tileRow}/{tileCol}`), PATH)).toBe(own);
    // The gateway names its own public base; the map still reads the page's host.
    expect(tileTemplate(item(`https://api.dev.example.com${PATH}/{tileMatrix}/{tileRow}/{tileCol}`), PATH)).toBe(own);
  });

  it("is refused off the tileset's path, with a query, in another axis order or type, or absent", () => {
    const refused = [
      item("https://broker:9090/x/views/air/ogc/features/collections/AirQualityObserved/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
      item("/api/endpoint/other/ogc/features/collections/AirQualityObserved/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
      item(`${PATH}/{tileMatrix}/{tileRow}/{tileCol}?f=mvt`),
      item(`${PATH}/{tileMatrix}/{tileCol}/{tileRow}`),
      item(`${PATH}/{tileMatrix}/{tileRow}/{tileCol}`, "application/json"),
      { links: [] },
      null,
    ];
    for (const tileset of refused) expect(tileTemplate(tileset, PATH)).toBeNull();
  });
});

describe("a tile read", () => {
  const bytes = new Uint8Array([0x1a, 0x02]).buffer;

  it("goes to the own host's Endpoint path with the session, asking for MVT", async () => {
    const fetchImpl = vi.fn(async () => new Response(bytes, { status: 200 }));
    const tile = await loadTile(`jc-tile://${PATH}/8/88/143`, undefined, fetchImpl as unknown as typeof fetch);
    expect(new Uint8Array(tile.data)).toEqual(new Uint8Array(bytes));
    expect(fetchImpl).toHaveBeenCalledWith(`${PATH}/8/88/143`, expect.objectContaining({ credentials: "same-origin", headers: { Accept: MVT } }));
  });

  it("draws nothing for an empty tile and fails on a refusal", async () => {
    const empty = vi.fn(async () => new Response(null, { status: 204 }));
    expect((await loadTile(`jc-tile://${PATH}/1/0/0`, undefined, empty as unknown as typeof fetch)).data.byteLength).toBe(0);
    const refused = vi.fn(async () => new Response("{}", { status: 403 }));
    await expect(loadTile(`jc-tile://${PATH}/1/0/0`, undefined, refused as unknown as typeof fetch)).rejects.toThrow("403");
  });

  it("never leaves the Endpoint: another host, another path or a climb out is not fetched", async () => {
    const fetchImpl = vi.fn();
    for (const url of ["jc-tile:////evil.example/api/endpoint/a/1/0/0", "jc-tile:///api/v1/projects/p/endpoints", "jc-tile:///api/endpoint/a/../../api/me", "https:///api/endpoint/a/1/0/0", "jc-tile:///api/endpoint/%2e%2e/v1/x", "jc-tile:///api/endpoint/a/.%2E/.%2e/me"]) {
      await expect(loadTile(url, undefined, fetchImpl as unknown as typeof fetch)).rejects.toThrow("outside the Endpoint");
    }
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

describe("a map drawing tiles", () => {
  it("adds the vector source under the page's shapes and selects a tile's shape by its id tag", async () => {
    const onSelect = vi.fn();
    render(<GeoView value={[]} onSelect={onSelect} tiles={{ template: `${PATH}/{z}/{y}/{x}`, layer: "AirQualityObserved" }} />);
    await waitFor(() => expect(added.sources.some(([id]) => id === "tiles")).toBe(true));
    const [, spec] = added.sources.find(([id]) => id === "tiles")!;
    expect(spec).toMatchObject({ type: "vector", tiles: [`jc-tile://${PATH}/{z}/{y}/{x}`], promoteId: "id" });
    const tileLayers = added.layers.filter(([layer]) => layer.source === "tiles");
    expect(tileLayers.map(([layer, before]) => [layer["source-layer"], before])).toEqual([
      ["AirQualityObserved", "areas"],
      ["AirQualityObserved", "areas"],
      ["AirQualityObserved", "areas"],
    ]);
    expect(protocols).toContain("jc-tile");
    expect(screen.getByText(TILED_NOTE)).toBeInTheDocument();
    clicks["tile-points"]({ features: [{ properties: { id: "urn:ngsi-ld:AirQualityObserved:9999" } }] });
    expect(onSelect).toHaveBeenCalledWith("urn:ngsi-ld:AirQualityObserved:9999");
  });
});

function stations(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    id: `urn:ngsi-ld:AirQualityObserved:${i}`,
    type: "AirQualityObserved",
    name: { type: "Property", value: `Station ${i}` },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [17 + i / 10_000, 48] } },
  }));
}

const config = parseGridConfig({
  source: { kind: "endpoint", slug: "air" },
  type: "AirQualityObserved",
  columns: [{ attr: "name", label: "Name", pinned: true }],
  pageSize: 10,
  map: { enabled: true, attr: "location" },
}).config!;

/** The fixture's rows, as the Endpoint source that names its tiles. */
const tiled = (n: number): EntitySource => ({ ...fixtureSource(stations(n)), tilesAt: "/api/endpoint/air" });

function tilesetFetch() {
  const fetchImpl = vi.fn(async (url: string) =>
    url === PATH ? new Response(JSON.stringify(item(`https://api.example.com${PATH}/{tileMatrix}/{tileRow}/{tileCol}`)), { status: 200, headers: { "content-type": "application/json" } }) : new Response(null, { status: 404 }),
  );
  vi.stubGlobal("fetch", fetchImpl);
  return fetchImpl;
}

describe("a grid's map switches to tiles past the count", () => {
  it("names its tiles only on an Endpoint source", () => {
    const transport = vi.fn();
    expect(endpointSource("air", transport).tilesAt).toBe("/api/endpoint/air");
  });

  it("draws the type's tiles once the Endpoint counts more than the threshold", async () => {
    const fetchImpl = tilesetFetch();
    render(<EntityGrid config={config} source={tiled(TILE_THRESHOLD + 1)} />);
    await waitFor(() => expect(added.sources.some(([id]) => id === "tiles")).toBe(true));
    expect(fetchImpl).toHaveBeenCalledWith(PATH, expect.objectContaining({ credentials: "same-origin" }));
  });

  it("stays on the page's shapes at or under the threshold", async () => {
    const fetchImpl = tilesetFetch();
    render(<EntityGrid config={config} source={tiled(TILE_THRESHOLD)} />);
    await screen.findByRole("button", { name: /Station 0/ });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(added.sources.some(([id]) => id === "tiles")).toBe(false);
  });

  it("stays on the page's shapes while a query narrows the answer, since a tile takes none", async () => {
    const fetchImpl = tilesetFetch();
    render(<EntityGrid config={config} source={tiled(TILE_THRESHOLD + 1)} query={{ q: "name==\"Station 1\"" }} />);
    await screen.findByRole("button", { name: /Station 0/ });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("opens the detail of a tile's entity that no loaded page holds, read by its URN", async () => {
    tilesetFetch();
    render(<EntityGrid config={config} source={tiled(TILE_THRESHOLD + 1)} />);
    await waitFor(() => expect(clicks["tile-points"]).toBeDefined());
    await waitFor(() => expect(added.sources.some(([id]) => id === "tiles")).toBe(true));
    clicks["tile-points"]({ features: [{ properties: { id: "urn:ngsi-ld:AirQualityObserved:1000" } }] });
    expect(await screen.findByRole("dialog")).toHaveTextContent("Station 1000");
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  });
});
