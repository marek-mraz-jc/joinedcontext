import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import type { Row } from "../ngsi";
import { columnKind, extent, format, pointOf } from "../ngsi";
import { mapWorkerReady, NO_BASEMAP, styleFor } from "../sdk/map";
import type { TileSource } from "../geo/GeoView";
import { TILED_NOTE } from "../geo/GeoView";
import { registerTileProtocol, tileUrl } from "../geo/tiles";

export { NO_BASEMAP, styleFor };

const SOURCE = "rows";
const TILES = "tiles";

const workerReady = mapWorkerReady();

/** One colour per distinct text value, a warm-to-cool ramp for numbers, the accent otherwise. */
export function colorOf(value: Row[string], range: [number, number] | null, accent: string): string {
  if (typeof value === "number" && range) {
    const t = range[1] === range[0] ? 0.5 : (value - range[0]) / (range[1] - range[0]);
    return `hsl(${Math.round(200 - t * 170)}, 70%, 45%)`;
  }
  if (typeof value === "string" && value !== "") {
    let hash = 0;
    for (const c of value) hash = (hash * 31 + c.charCodeAt(0)) % 360;
    return `hsl(${hash}, 65%, 42%)`;
  }
  return accent;
}

export interface Feature {
  type: "Feature";
  id: string;
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { id: string; label: string; color: string };
}

/** The rows with a location, as the one GeoJSON source the map draws. */
export function featureCollection(rows: Row[], location: string, label: string | undefined, color: string | undefined, accent: string): { type: "FeatureCollection"; features: Feature[] } {
  const range = color && columnKind(rows, color) === "number" ? extent(rows, color) : null;
  const features: Feature[] = [];
  for (const row of rows) {
    const point = pointOf(row[location]);
    if (!point) continue;
    features.push({
      type: "Feature",
      id: row.id,
      geometry: { type: "Point", coordinates: point },
      properties: { id: row.id, label: label ? format(row[label]) : row.id, color: color ? colorOf(row[color], range, accent) : accent },
    });
  }
  return { type: "FeatureCollection", features };
}

/** The map is built once; every later set of rows reaches it through `setData`. */
export function MapView({
  rows,
  location,
  label,
  color,
  accent,
  selected,
  onSelect,
  basemap,
  tiles,
}: {
  rows: Row[];
  location: string;
  label?: string;
  color?: string;
  accent: string;
  selected: string | null;
  onSelect: (id: string) => void;
  basemap?: string;
  /** Past the rows a source reads, every entity from the Endpoint's tiles, under the rows (T-5195). */
  tiles?: TileSource | null;
}) {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const select = useRef(onSelect);
  select.current = onSelect;
  const collection = useMemo(() => featureCollection(rows, location, label, color, accent), [rows, location, label, color, accent]);
  const style = useMemo(() => styleFor(basemap), [basemap]);

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;
    void workerReady.then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style,
        center: [0, 0],
        zoom: 1,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      // For a person debugging a screenshot: the map instance, reachable from the console.
      (window as unknown as { kitMap?: MapLibreMap }).kitMap = instance;
      wire(instance);
    });
    const wire = (instance: MapLibreMap): void => {
      // The library's own failures (a tile, the worker, the style) are said out loud; a silent
      // map is the hardest kind to debug from a screenshot.
      instance.on("error", (event) => {
        console.error("kit: map error", event.error?.message ?? event);
      });
    instance.on("load", () => {
      instance.addSource(SOURCE, { type: "geojson", data: { type: "FeatureCollection", features: [] } });
      instance.addLayer({
        id: "dots",
        type: "circle",
        source: SOURCE,
        paint: { "circle-radius": 6, "circle-color": ["get", "color"], "circle-stroke-width": 1.5, "circle-stroke-color": "#ffffff" },
      });
      instance.on("click", "tile-dots", (event) => {
        const id = event.features?.[0]?.properties?.id;
        if (typeof id === "string") select.current(id);
      });
      instance.on("click", "dots", (event) => {
        const id = event.features?.[0]?.properties?.id;
        if (typeof id === "string") select.current(id);
      });
      setReady(true);
    });
    };
    return () => {
      gone = true;
      instance?.remove();
      map.current = null;
    };
  }, []);

  // The tiles go under the rows, so a row keeps its colour and a tile's shape is told apart by its
  // `id` tag, the entity's URN.
  const template = tiles?.template;
  const layer = tiles?.layer;
  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance || !template || !layer) return;
    registerTileProtocol();
    instance.addSource(TILES, { type: "vector", tiles: [tileUrl(template)], minzoom: 0, maxzoom: 16, promoteId: "id" });
    instance.addLayer(
      {
        id: "tile-dots",
        type: "circle",
        source: TILES,
        "source-layer": layer,
        filter: ["match", ["geometry-type"], ["Point", "MultiPoint"], true, false],
        paint: { "circle-radius": 5, "circle-color": accent, "circle-stroke-width": 1, "circle-stroke-color": "#ffffff" },
      },
      "dots",
    );
    return () => {
      if (instance.getLayer?.("tile-dots")) instance.removeLayer("tile-dots");
      if (instance.getSource?.(TILES)) instance.removeSource(TILES);
    };
  }, [ready, template, layer, accent]);

  const fitted = useRef(false);
  useEffect(() => {
    if (!ready) return;
    const source = map.current?.getSource(SOURCE) as GeoJSONSource | undefined;
    source?.setData(collection);
    if (!fitted.current && collection.features.length > 0 && map.current?.fitBounds) {
      fitted.current = true;
      const xs = collection.features.map((f) => f.geometry.coordinates[0]);
      const ys = collection.features.map((f) => f.geometry.coordinates[1]);
      map.current.fitBounds([[Math.min(...xs), Math.min(...ys)], [Math.max(...xs), Math.max(...ys)]], { padding: 32, maxZoom: 14, duration: 0 });
    }
  }, [collection, ready]);

  const chosen = collection.features.find((f) => f.id === selected);
  return (
    <div className="map-wrap">
      <div className="map" ref={container} data-testid="map" role="application" aria-label="Map" />
      {!basemap && <span className="map-notice">{NO_BASEMAP}</span>}
      <span className="map-count">
        {tiles ? TILED_NOTE : `${collection.features.length} on the map`}
        {chosen ? ` · ${chosen.properties.label}` : selected && tiles ? ` · ${selected}` : ""}
      </span>
    </div>
  );
}
