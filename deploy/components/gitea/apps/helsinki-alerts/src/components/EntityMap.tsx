import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource, IControl } from "maplibre-gl";
import { MapboxOverlay } from "@deck.gl/mapbox";
import { ScatterplotLayer } from "@deck.gl/layers";
import { GridLayer, HexagonLayer } from "@deck.gl/aggregation-layers";

import { currentTokens, displayName, extent, format, mapWorkerReady, NO_BASEMAP, NO_LOCATIONS, pointOf, styleFor, toFeatureCollection, useClient } from "@joinedcontext/sdk";
import type { Cell, DesignTokens, Geo, Row } from "@joinedcontext/sdk";

export const DECK_THRESHOLD = 50_000;
export type MapMode = "auto" | "points" | "hexbin" | "grid";

export function renderPath(count: number, mode: MapMode): "maplibre" | "deck-points" | "deck-hexbin" | "deck-grid" {
  if (mode === "hexbin") return "deck-hexbin";
  if (mode === "grid") return "deck-grid";
  if (mode === "points") return "maplibre";
  return count >= DECK_THRESHOLD ? "deck-points" : "maplibre";
}

function parseHexColor(hex: string): [number, number, number] {
  let cleaned = hex.replace(/^#/, "").trim();
  if (cleaned.length === 3) {
    cleaned = cleaned
      .split("")
      .map((c) => c + c)
      .join("");
  }
  if (cleaned.length !== 6) {
    return [0, 0, 0];
  }
  const num = parseInt(cleaned, 16);
  return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
}

function toHex(r: number, g: number, b: number): string {
  const clamp = (n: number) => Math.max(0, Math.min(255, Math.round(n)));
  const to2 = (n: number) => clamp(n).toString(16).padStart(2, "0");
  return `#${to2(r)}${to2(g)}${to2(b)}`;
}

export function colorRamp(value: Cell, range: [number, number] | null, tokens?: DesignTokens): string {
  const tks = tokens ?? currentTokens();
  if (typeof value !== "number" || !range || range[1] <= range[0] || Number.isNaN(value)) {
    return tks.map.point;
  }
  const t = Math.max(0, Math.min(1, (value - range[0]) / (range[1] - range[0])));
  const [r0, g0, b0] = parseHexColor(tks.map.low);
  const [r1, g1, b1] = parseHexColor(tks.map.high);
  return toHex(r0 + t * (r1 - r0), g0 + t * (g1 - g0), b0 + t * (b1 - b0));
}

function deckColorRange(tokens: DesignTokens): [number, number, number][] {
  const [r0, g0, b0] = parseHexColor(tokens.map.low);
  const [r1, g1, b1] = parseHexColor(tokens.map.high);
  const steps: [number, number, number][] = [];
  for (let i = 0; i < 6; i++) {
    const t = i / 5;
    steps.push([Math.round(r0 + t * (r1 - r0)), Math.round(g0 + t * (g1 - g0)), Math.round(b0 + t * (b1 - b0))]);
  }
  return steps;
}

/** The label of a row on the map: the `label` attribute when it holds text, else its display name. */
function nameOf(row: Row, label?: string): string {
  const text = label ? format(row[label]).trim() : "";
  return text === "" ? displayName(row) : text;
}

interface PointRow {
  position: [number, number];
  rgb: [number, number, number];
  row: Row;
}


function extractCoordinates(geometry: Geo): Array<[number, number]> {
  const coords: Array<[number, number]> = [];
  function dig(val: unknown) {
    if (Array.isArray(val)) {
      if (val.length >= 2 && typeof val[0] === "number" && typeof val[1] === "number") {
        coords.push([val[0], val[1]]);
      } else {
        for (const item of val) dig(item);
      }
    }
  }
  dig(geometry.coordinates);
  return coords;
}

export function EntityMap({
  rows,
  location = "location",
  label,
  color,
  selected = null,
  onSelect,
  basemap,
  mode = "auto",
  height,
  radius = 500,
}: {
  rows: Row[];
  location?: string;
  label?: string;
  color?: string;
  selected?: string | null;
  onSelect?: (row: Row) => void;
  basemap?: string;
  mode?: MapMode;
  height?: number;
  radius?: number;
}): React.JSX.Element {
  const client = useClient();
  const effectiveBasemap = basemap ?? client.config.basemap;
  const tokens = currentTokens();

  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const overlayRef = useRef<MapboxOverlay | null>(null);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);

  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;

  const path = renderPath(rows.length, mode);

  const ext = useMemo(() => (color ? extent(rows, color) : null), [rows, color]);

  const rowById = useMemo(() => {
    const map = new Map<string, Row>();
    for (const r of rows) map.set(r.id, r);
    return map;
  }, [rows]);

  const rowByIdRef = useRef(rowById);
  rowByIdRef.current = rowById;

  const collection = useMemo(() => {
    const base = toFeatureCollection(rows, location, [...(label ? [label] : []), ...(color ? [color] : [])]);
    const features = base.features.map((f) => {
      const r = rowById.get(f.id);
      const c = color && r ? colorRamp(r[color], ext, tokens) : tokens.map.point;
      const l = r ? nameOf(r, label) : f.id;
      return {
        ...f,
        properties: {
          ...f.properties,
          label: l,
          color: c,
        },
      };
    });
    return { type: "FeatureCollection" as const, features };
  }, [rows, location, label, color, ext, tokens, rowById]);

  const pointRows: PointRow[] = useMemo(() => {
    const pts: PointRow[] = [];
    for (const row of rows) {
      const pt = pointOf(row[location]);
      if (!pt) continue;
      const hex = color ? colorRamp(row[color], ext, tokens) : tokens.map.point;
      pts.push({
        position: pt,
        rgb: parseHexColor(hex),
        row,
      });
    }
    return pts;
  }, [rows, location, color, ext, tokens]);

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;

    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(effectiveBasemap),
        center: [0, 0],
        zoom: 1,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;

      instance.on("error", (event) => {
        console.error("jc: map error", event.error?.message ?? event);
      });

      instance.on("load", () => {
        if (gone) return;
        instance!.addSource("jc-rows", {
          type: "geojson",
          data: { type: "FeatureCollection", features: [] },
        });

        instance!.addLayer({
          id: "jc-fill",
          type: "fill",
          source: "jc-rows",
          filter: ["in", ["geometry-type"], ["literal", ["Polygon", "MultiPolygon"]]],
          paint: { "fill-opacity": 0.35, "fill-color": ["get", "color"] },
        });

        instance!.addLayer({
          id: "jc-line",
          type: "line",
          source: "jc-rows",
          filter: ["in", ["geometry-type"], ["literal", ["LineString", "MultiLineString"]]],
          paint: { "line-color": ["get", "color"], "line-width": 2 },
        });

        instance!.addLayer({
          id: "jc-points",
          type: "circle",
          source: "jc-rows",
          filter: ["in", ["geometry-type"], ["literal", ["Point", "MultiPoint"]]],
          paint: {
            "circle-radius": 6,
            "circle-color": ["case", ["==", ["get", "id"], selected ?? ""], tokens.map.selected, ["get", "color"]],
            "circle-stroke-color": tokens.map.stroke,
            "circle-stroke-width": 1.5,
          },
        });

        const handleClick = (e: { features?: Array<{ properties?: { id?: string } }> }) => {
          const id = e.features?.[0]?.properties?.id;
          if (typeof id === "string") {
            const r = rowByIdRef.current.get(id);
            if (r) onSelectRef.current?.(r);
          }
        };

        instance!.on("click", "jc-points", handleClick);
        instance!.on("click", "jc-fill", handleClick);
        instance!.on("click", "jc-line", handleClick);

        setReady(true);
      });
    });

    return () => {
      gone = true;
      if (overlayRef.current && map.current) {
        map.current.removeControl(overlayRef.current as unknown as IControl);
        overlayRef.current = null;
      }
      instance?.remove();
      map.current = null;
    };
  }, []);

  useEffect(() => {
    if (!ready || !map.current) return;
    map.current.setPaintProperty("jc-points", "circle-color", [
      "case",
      ["==", ["get", "id"], selected ?? ""],
      tokens.map.selected,
      ["get", "color"],
    ]);
  }, [ready, selected, tokens.map.selected]);

  useEffect(() => {
    if (!ready || !map.current) return;

    if (path.startsWith("deck-")) {
      const src = map.current.getSource("jc-rows") as GeoJSONSource | undefined;
      src?.setData({ type: "FeatureCollection", features: [] });

      let layer;
      if (path === "deck-points") {
        layer = new ScatterplotLayer({
          id: "jc-scatter",
          data: pointRows,
          getPosition: (d: PointRow) => d.position,
          getFillColor: (d: PointRow) => d.rgb,
          radiusMinPixels: 2,
          radiusMaxPixels: 8,
          pickable: true,
          onClick: (info: { object?: PointRow }) => info.object && onSelectRef.current?.(info.object.row),
        });
      } else if (path === "deck-hexbin") {
        layer = new HexagonLayer({
          id: "jc-hexagon",
          data: pointRows,
          getPosition: (d: PointRow) => d.position,
          radius,
          pickable: true,
          extruded: false,
          colorRange: deckColorRange(tokens),
        });
      } else {
        layer = new GridLayer({
          id: "jc-grid",
          data: pointRows,
          getPosition: (d: PointRow) => d.position,
          cellSize: radius,
          pickable: true,
          colorRange: deckColorRange(tokens),
        });
      }

      if (!overlayRef.current) {
        const overlay = new MapboxOverlay({ interleaved: false, layers: [layer] });
        overlayRef.current = overlay;
        map.current.addControl(overlay as unknown as IControl);
      } else {
        overlayRef.current.setProps({ layers: [layer] });
      }
    } else {
      if (overlayRef.current) {
        map.current.removeControl(overlayRef.current as unknown as IControl);
        overlayRef.current = null;
      }
      const src = map.current.getSource("jc-rows") as GeoJSONSource | undefined;
      src?.setData(collection as GeoJSON.FeatureCollection);
    }

    if (!fitted.current && collection.features.length > 0 && map.current.fitBounds) {
      fitted.current = true;
      // A loop, not Math.min(...xs): spreading 50 000 coordinates overflows the call stack.
      let [west, south, east, north] = [Infinity, Infinity, -Infinity, -Infinity];
      for (const f of collection.features) {
        for (const [x, y] of extractCoordinates(f.geometry)) {
          west = Math.min(west, x);
          south = Math.min(south, y);
          east = Math.max(east, x);
          north = Math.max(north, y);
        }
      }
      if (west <= east) {
        map.current.fitBounds(
          [
            [west, south],
            [east, north],
          ],
          { padding: 32, maxZoom: 14, duration: 0 },
        );
      }
    }
  }, [ready, path, collection, pointRows, radius, tokens]);

  const n = path === "maplibre" ? collection.features.length : pointRows.length;
  const chosen = selected ? rowById.get(selected) : null;
  const selectedLabel = chosen ? ` · ${nameOf(chosen, label)}` : "";

  return (
    <div className="jc-map" style={height === undefined ? undefined : { height }}>
      <div className="jc-map-canvas" ref={container} data-testid="jc-map" role="application" aria-label="Map" />
      {!effectiveBasemap && <span className="jc-map-notice">{NO_BASEMAP}</span>}
      {n === 0 && rows.length > 0 ? (
        <span className="jc-map-count">{NO_LOCATIONS}</span>
      ) : (
        <span className="jc-map-count">
          {n} on the map{selectedLabel}
        </span>
      )}
    </div>
  );
}
