import { useEffect, useRef, useState } from "react";
import { LngLatBounds, Map as MapLibreMap, Popup } from "maplibre-gl";
import type { GeoJSONSource, MapGeoJSONFeature, MapMouseEvent } from "maplibre-gl";
import { currentTokens, mapWorkerReady, NO_BASEMAP, styleFor, useClient } from "@joinedcontext/sdk";

/** One point on the map: where, in which colour, and the lines its popup shows (text only). */
export interface MapPoint {
  id: string;
  at: [number, number];
  color: string;
  lines: string[];
}

/**
 * A MapLibre map of `points`, with `line` drawn under them when given (a route). It fits the
 * points once, when they first arrive, so a refresh never moves the reader's view; a click on a
 * point opens its popup and calls `onPick`.
 */
export function MapView({
  points,
  line,
  label,
  onPick,
  height = 420,
}: {
  points: MapPoint[];
  line?: Array<[number, number]>;
  /** What the map shows, for a screen reader: the list beside it carries the same. */
  label: string;
  onPick?: (id: string) => void;
  height?: number;
}): React.JSX.Element {
  const client = useClient();
  const basemap = client.config.basemap;
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);
  const pointsRef = useRef(points);
  pointsRef.current = points;
  const onPickRef = useRef(onPick);
  onPickRef.current = onPick;

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;
    void mapWorkerReady().then(() => {
      if (gone || !container.current) return;
      const tokens = currentTokens();
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(basemap),
        center: [24.94, 60.17],
        zoom: 10,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      instance.on("error", (event) => console.error("map error", event.error?.message ?? event));
      instance.on("load", () => {
        if (gone || !instance) return;
        instance.addSource("app-line", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        instance.addSource("app-points", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        instance.addLayer({
          id: "app-line",
          type: "line",
          source: "app-line",
          paint: { "line-color": tokens.color.accent, "line-width": 3, "line-opacity": 0.8 },
        });
        instance.addLayer({
          id: "app-points",
          type: "circle",
          source: "app-points",
          paint: {
            "circle-radius": 6,
            "circle-color": ["get", "color"],
            "circle-stroke-color": tokens.map.stroke,
            "circle-stroke-width": 1.5,
          },
        });
        instance.on("click", "app-points", (event: MapMouseEvent & { features?: MapGeoJSONFeature[] }) => {
          const id: unknown = event.features?.[0]?.properties?.id;
          const point = pointsRef.current.find((p) => p.id === id);
          if (!point || !instance) return;
          onPickRef.current?.(point.id);
          // Text nodes only: an entity's values never reach the page as markup.
          const body = document.createElement("div");
          body.className = "jc-map-popup";
          for (const [index, text] of point.lines.entries()) {
            const element = document.createElement(index === 0 ? "strong" : "p");
            element.textContent = text;
            body.append(element);
          }
          new Popup({ closeButton: true, maxWidth: "260px" }).setLngLat(point.at).setDOMContent(body).addTo(instance);
        });
        setReady(true);
      });
    });
    return () => {
      gone = true;
      instance?.remove();
      map.current = null;
    };
  }, [basemap]);

  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;
    (instance.getSource("app-points") as GeoJSONSource | undefined)?.setData({
      type: "FeatureCollection",
      features: points.map((p) => ({
        type: "Feature",
        id: p.id,
        properties: { id: p.id, color: p.color },
        geometry: { type: "Point", coordinates: p.at },
      })),
    });
    (instance.getSource("app-line") as GeoJSONSource | undefined)?.setData({
      type: "FeatureCollection",
      features: line && line.length > 1 ? [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: line } }] : [],
    });
    if (!fitted.current && points.length > 0) {
      fitted.current = true;
      const bounds = new LngLatBounds();
      for (const p of points) bounds.extend(p.at);
      instance.fitBounds(bounds, { padding: 32, maxZoom: 15, duration: 0 });
    }
  }, [ready, points, line]);

  return (
    <div className="jc-map" data-testid="jc-map" style={{ height }}>
      <div className="jc-map-canvas" ref={container} role="application" aria-label={label} />
      {!basemap && <span className="jc-map-notice">{NO_BASEMAP}</span>}
    </div>
  );
}
