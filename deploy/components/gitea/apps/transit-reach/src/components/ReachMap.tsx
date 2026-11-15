import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap, Popup } from "maplibre-gl";
import type { ExpressionSpecification, GeoJSONSource, MapGeoJSONFeature, MapMouseEvent } from "maplibre-gl";
import { mapWorkerReady, NO_BASEMAP, styleFor, useClient } from "@joinedcontext/sdk";
import type { Cell, StopOut } from "../analysis";
import { reachColours } from "../theme";

/**
 * The area reached on one map (T-3331): each hexagon in its band's colour (`near`, `mid`, `far` of
 * the scheme's `REACH` colours for the first three bands), the derived stops as dots, and the starting
 * point. A click on a stop names it in a popup built of text; a click anywhere else starts from
 * there.
 */
export function ReachMap({
  cells,
  stops,
  origin,
  bands,
  label,
  describe,
  onPick,
  height = 480,
}: {
  cells: Cell[];
  stops: StopOut[];
  origin: { lon: number; lat: number };
  /** The bands' minutes, ascending. */
  bands: number[];
  /** What a screen reader calls the map. */
  label: string;
  /** The popup's lines for a stop. */
  describe: (stop: StopOut, index: number) => string[];
  /** Called with the point a person clicks. */
  onPick: (point: { lon: number; lat: number }) => void;
  height?: number;
}): React.JSX.Element {
  const client = useClient();
  const basemap = client.config.basemap;
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const describeRef = useRef(describe);
  describeRef.current = describe;
  const onPickRef = useRef(onPick);
  onPickRef.current = onPick;
  const tokens = useMemo(() => ({ map: reachColours() }), []);
  const lookup = useRef({ stops });
  lookup.current = { stops };

  const cellData = useMemo(
    () => ({
      type: "FeatureCollection" as const,
      features: cells.map((cell, index) => ({
        type: "Feature" as const,
        id: index,
        properties: { index, band: cell.band },
        geometry: { type: "Polygon" as const, coordinates: [cell.ring] },
      })),
    }),
    [cells],
  );
  const stopData = useMemo(
    () => ({
      type: "FeatureCollection" as const,
      features: stops.map((stop, index) => ({
        type: "Feature" as const,
        id: index,
        properties: { index, reached: stop.minutes !== null },
        geometry: { type: "Point" as const, coordinates: [stop.lon, stop.lat] },
      })),
    }),
    [stops],
  );
  const originData = useMemo(
    () => ({ type: "Feature" as const, properties: {}, geometry: { type: "Point" as const, coordinates: [origin.lon, origin.lat] } }),
    [origin],
  );

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;
    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(basemap),
        center: [24.94, 60.19],
        zoom: 11,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      // The page lays out after the map is built: the canvas follows its box, not its first size.
      const box = container.current;
      const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(() => instance?.resize());
      observer?.observe(box);
      instance.on("remove", () => observer?.disconnect());
      instance.on("error", (event) => console.error("transit-reach: map error", event.error?.message ?? event));
      instance.on("load", () => {
        if (gone || !instance) return;
        const empty = { type: "FeatureCollection" as const, features: [] };
        instance.addSource("cells", { type: "geojson", data: empty });
        instance.addSource("stops", { type: "geojson", data: empty });
        instance.addSource("origin", { type: "geojson", data: empty });
        instance.addLayer({
          id: "cells",
          type: "fill",
          source: "cells",
          paint: { "fill-color": tokens.map.far, "fill-opacity": 0.45, "fill-outline-color": tokens.map.stroke },
        });
        instance.addLayer({
          id: "stops",
          type: "circle",
          source: "stops",
          paint: {
            "circle-color": tokens.map.stop,
            "circle-opacity": ["case", ["get", "reached"], 1, 0.4],
            "circle-stroke-color": tokens.map.stroke,
            "circle-stroke-width": 1,
            "circle-radius": 4,
          },
        });
        instance.addLayer({
          id: "origin",
          type: "circle",
          source: "origin",
          paint: { "circle-color": tokens.map.origin, "circle-stroke-color": tokens.map.stroke, "circle-stroke-width": 2, "circle-radius": 8 },
        });
        const popup = (event: MapMouseEvent, lines: string[]) => {
          if (!instance) return;
          const body = document.createElement("div");
          for (const line of lines) {
            const p = document.createElement("p");
            p.textContent = line;
            body.append(p);
          }
          new Popup({ closeButton: true }).setLngLat(event.lngLat).setDOMContent(body).addTo(instance);
        };
        const indexOf = (features: MapGeoJSONFeature[] | undefined) => features?.[0]?.properties?.index;
        instance.on("click", (event) => {
          if (!instance) return;
          const onStop = instance.queryRenderedFeatures(event.point, { layers: ["stops"] });
          const stopIndex = indexOf(onStop);
          if (typeof stopIndex === "number" && lookup.current.stops[stopIndex]) {
            popup(event, describeRef.current(lookup.current.stops[stopIndex], stopIndex));
            return;
          }
          // Anywhere else starts from there.
          onPickRef.current({ lon: event.lngLat.lng, lat: event.lngLat.lat });
        });
        setReady(true);
      });
    });
    return () => {
      gone = true;
      instance?.remove();
      map.current = null;
    };
    // The map is built once; its data and colours reach it through the effect below.
  }, [basemap, tokens]);

  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;
    (instance.getSource("cells") as GeoJSONSource | undefined)?.setData(cellData);
    (instance.getSource("stops") as GeoJSONSource | undefined)?.setData(stopData);
    (instance.getSource("origin") as GeoJSONSource | undefined)?.setData(originData);
    // The bands in order, near to far; a fourth or later band takes the far colour.
    const colours = [tokens.map.near, tokens.map.mid, tokens.map.far];
    const steps = [
      "step",
      ["get", "band"],
      colours[0],
      ...bands.slice(0, -1).flatMap((minutes, i) => [minutes + 0.001, colours[Math.min(i + 1, colours.length - 1)]]),
    ] as ExpressionSpecification;
    instance.setPaintProperty("cells", "fill-color", steps);
  }, [ready, cellData, stopData, originData, bands, tokens]);

  // The whole reach in view: the cells' extent, else the starting point.
  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;
    if (cells.length === 0) {
      instance.easeTo({ center: [origin.lon, origin.lat], duration: 0 });
      return;
    }
    const lons = cells.flatMap((cell) => cell.ring.map((p) => p[0]));
    const lats = cells.flatMap((cell) => cell.ring.map((p) => p[1]));
    instance.fitBounds(
      [
        [Math.min(...lons), Math.min(...lats)],
        [Math.max(...lons), Math.max(...lats)],
      ],
      { padding: 24, maxZoom: 14, duration: 0 },
    );
  }, [ready, cells, origin]);

  return (
    <div className="jc-map" style={{ height }}>
      <div className="jc-map-canvas" ref={container} data-testid="jc-map" role="application" aria-label={label} />
      {!basemap && <span className="jc-map-notice">{NO_BASEMAP}</span>}
    </div>
  );
}
